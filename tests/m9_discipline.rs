use devcode::agent::model::{Message, Response, Role, ToolCall, Usage};
use devcode::agent::scripted::ScriptedModel;
use devcode::agent::tool::{Tool, ToolRegistry};
use devcode::agent::{run_agent_loop, ExitReason, LoopLimits};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::discipline::{
    apply_disciplined_file_edit, compact_messages, has_dependency_addition, is_dependency_manifest,
    should_compact, CompactionContext, DeclareScopeTool, ScopeContract, TaskSummary,
};
use devcode::gate::tool_log::{ToolKind, ToolLog};
use devcode::manifest::record::{FileType, PathRecord, StepManifest};
use devcode::profile::Profile;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

#[test]
fn compaction_keeps_complete_tool_group_and_unicode() {
    let config = Config::default();
    let log = ToolLog::new();
    let ctx = CompactionContext {
        context_window: 1000,
        active_manifest: None,
        current_diff: "",
        config: &config,
        tool_log: &log,
    };
    let calls: Vec<ToolCall> = (0..3)
        .map(|i| ToolCall {
            id: i.to_string(),
            name: "read_file".into(),
            arguments: "{}".into(),
            thought_signature: None,
        })
        .collect();
    let messages = vec![
        Message::user("goal"),
        Message::assistant("", calls),
        Message::tool("é".repeat(700), "0"),
        Message::tool("two", "1"),
        Message::tool("three", "2"),
    ];
    let compacted = compact_messages(&messages, &ctx);
    assert_eq!(compacted[2].tool_calls.len(), 3);
    assert_eq!(compacted[3].tool_call_id.as_deref(), Some("0"));
    assert_eq!(compacted[5].tool_call_id.as_deref(), Some("2"));
    assert!(compacted[3].content.contains("output truncated"));
}

#[test]
fn test_scope_contract_in_scope_and_out_of_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let scope = ScopeContract::new();
    let tool = DeclareScopeTool::new(scope.clone());

    // Execute declare_scope tool
    let args = serde_json::json!({
        "files": ["src/in_scope.rs"],
        "reason": "implement feature X"
    })
    .to_string();

    let res = tool.execute(&args).expect("declare_scope should succeed");
    assert!(res.contains("Scope declared: 1 file(s)"));
    assert_eq!(scope.get_reason(), "implement feature X");

    assert!(scope.is_in_scope(Path::new("src/in_scope.rs"), root));
    assert!(!scope.is_in_scope(Path::new("src/out_of_scope.rs"), root));

    let mut tool_log = ToolLog::new();
    let mut summary = TaskSummary::default();

    // 1. In-scope edit with auto-approve does NOT prompt
    {
        let input = Cursor::new(b"");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

        let ok = apply_disciplined_file_edit(
            root,
            Path::new("src/in_scope.rs"),
            "fn in_scope() {}\n",
            "session_scope",
            1,
            &scope,
            &mut prompt,
            &mut tool_log,
            &mut summary,
        )
        .expect("in-scope edit failed");

        assert!(ok, "in-scope edit should be approved without prompt");
        assert!(root.join("src/in_scope.rs").exists());
        assert_eq!(summary.files_outside_scope, 0);
        assert_eq!(summary.files_edited, 1);
    }

    // 2. Out-of-scope edit prompts with "outside declared scope" even with auto-approve
    {
        // User rejects out-of-scope edit
        let input = Cursor::new(b"n\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

        let ok = apply_disciplined_file_edit(
            root,
            Path::new("src/out_of_scope.rs"),
            "fn out_of_scope() {}\n",
            "session_scope",
            2,
            &scope,
            &mut prompt,
            &mut tool_log,
            &mut summary,
        )
        .expect("out-of-scope edit failed");

        assert!(
            !ok,
            "out-of-scope edit should be rejected when user says no"
        );
        assert!(!root.join("src/out_of_scope.rs").exists());
        assert_eq!(summary.files_outside_scope, 1);

        let prompt_output = String::from_utf8_lossy(&output);
        assert!(
            prompt_output.contains("outside declared scope"),
            "Prompt must state 'outside declared scope', got: {}",
            prompt_output
        );
    }

    // 3. Out-of-scope edit approved by user
    {
        let input = Cursor::new(b"yes\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

        let ok = apply_disciplined_file_edit(
            root,
            Path::new("src/out_of_scope.rs"),
            "fn out_of_scope() {}\n",
            "session_scope",
            3,
            &scope,
            &mut prompt,
            &mut tool_log,
            &mut summary,
        )
        .expect("out-of-scope edit failed");

        assert!(ok, "out-of-scope edit should succeed when user approves");
        assert!(root.join("src/out_of_scope.rs").exists());
        assert_eq!(summary.files_outside_scope, 2);
        assert_eq!(summary.files_edited, 2);
    }
}

#[test]
fn test_dependency_addition_prompts_and_logs_justification() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let scope = ScopeContract::new();
    scope.declare(
        vec![PathBuf::from("Cargo.toml")],
        "manage dependencies".to_string(),
    );

    let old_cargo = "[package]\nname = \"my-pkg\"\nversion = \"0.1.0\"\n";
    let new_cargo =
        "[package]\nname = \"my-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n";

    fs::write(root.join("Cargo.toml"), old_cargo).unwrap();

    assert!(is_dependency_manifest(Path::new("Cargo.toml")));
    assert!(has_dependency_addition(
        Path::new("Cargo.toml"),
        old_cargo,
        new_cargo
    ));

    // Non-dependency edit does not flag
    let version_bump_cargo = "[package]\nname = \"my-pkg\"\nversion = \"0.2.0\"\n";
    assert!(!has_dependency_addition(
        Path::new("Cargo.toml"),
        old_cargo,
        version_bump_cargo
    ));

    // Verify other dependency manifests
    assert!(is_dependency_manifest(Path::new("package.json")));
    assert!(is_dependency_manifest(Path::new("pyproject.toml")));
    assert!(is_dependency_manifest(Path::new("go.mod")));
    assert!(is_dependency_manifest(Path::new("requirements.txt")));
    assert!(is_dependency_manifest(Path::new("requirements-dev.txt")));

    let mut tool_log = ToolLog::new();
    let mut summary = TaskSummary::default();

    // User provides justification for adding dependency
    let input = Cursor::new(b"serde needed for json config\n");
    let mut output = Vec::new();
    let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

    let ok = apply_disciplined_file_edit(
        root,
        Path::new("Cargo.toml"),
        new_cargo,
        "session_dep",
        1,
        &scope,
        &mut prompt,
        &mut tool_log,
        &mut summary,
    )
    .expect("dependency edit failed");

    assert!(ok, "dependency edit should be approved with justification");
    assert_eq!(summary.dependencies_added, 1);

    // Verify justification was recorded in ToolLog
    let just_entry = tool_log
        .entries
        .iter()
        .find(|e| e.kind == ToolKind::DependencyJustification)
        .expect("justification must be logged in ToolLog");

    assert!(just_entry.detail.contains("Cargo.toml"));
    assert!(just_entry.detail.contains("serde needed for json config"));
}

#[test]
fn test_compaction_triggers_at_threshold_and_preserves_pinned_and_test_result() {
    let context_window = 1000;

    // Threshold check (> 75%)
    assert!(!should_compact(700, context_window));
    assert!(!should_compact(750, context_window));
    assert!(should_compact(751, context_window));
    assert!(should_compact(900, context_window));

    let mut config = Config::default();
    config.verification.test_command = Some("cargo test --quiet".to_string());

    let mut tool_log = ToolLog::new();
    tool_log.record_file_edit("src/lib.rs", 100);
    tool_log.record_file_edit("src/main.rs", 150);
    let test_cmd_verbatim = "cargo test --quiet";
    tool_log.record_command(test_cmd_verbatim, Some(0), 200);

    let manifest = StepManifest {
        session_id: "compact_sess_1".to_string(),
        step_id: 4,
        timestamp: 200,
        pre_state: vec![PathRecord {
            path: PathBuf::from("src/lib.rs"),
            exists: true,
            file_type: Some(FileType::File),
            symlink_target: None,
            mode: Some(0o644),
            sha256: Some("sha_pre".to_string()),
            blob_ref: Some("sha_pre".to_string()),
        }],
        post_state: Some(vec![PathRecord {
            path: PathBuf::from("src/lib.rs"),
            exists: true,
            file_type: Some(FileType::File),
            symlink_target: None,
            mode: Some(0o644),
            sha256: Some("sha_post".to_string()),
            blob_ref: Some("sha_post".to_string()),
        }]),
    };

    let current_diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";

    let ctx = CompactionContext {
        context_window,
        active_manifest: Some(&manifest),
        current_diff,
        config: &config,
        tool_log: &tool_log,
    };

    let long_tool_output = format!(
        "start_head: {}\nend_tail: {}",
        "x".repeat(600),
        "y".repeat(600)
    );

    let messages = vec![
        Message::user("Implement disciplined feature"),
        Message::assistant("I will inspect the files", Vec::new()),
        Message::tool("intermediate observation 1", "call_1"),
        Message::assistant("Now reading larger data", Vec::new()),
        Message::tool(&long_tool_output, "call_2"),
    ];

    let compacted = compact_messages(&messages, &ctx);

    assert_eq!(compacted[0].role, Role::User);
    assert_eq!(compacted[0].content, "Implement disciplined feature");

    let summary_msg = &compacted[1];
    assert_eq!(summary_msg.role, Role::Assistant);
    assert!(summary_msg
        .content
        .contains("--- HARNESS DETERMINISTIC COMPACTION SUMMARY ---"));

    // 1. Pinned config preserved
    assert!(summary_msg.content.contains("cargo test --quiet"));

    // 2. Pinned active manifest preserved
    assert!(summary_msg.content.contains("compact_sess_1"));
    assert!(summary_msg.content.contains("Step: 4"));

    // 3. Pinned current diff preserved
    assert!(summary_msg.content.contains(current_diff));

    // 4. Last test result preserved verbatim
    let expected_test_record = format!(
        "Command: '{}', Exit Code: Some(0), Timestamp: 200",
        test_cmd_verbatim
    );
    assert!(
        summary_msg.content.contains(&expected_test_record),
        "Compacted summary must preserve last test result verbatim"
    );

    // 5. Long tool output in retained messages truncated
    let last_msg = compacted.last().unwrap();
    assert!(last_msg
        .content
        .contains("[... output truncated by compaction ...]"));
}

#[test]
fn test_end_of_task_summary_line_matches_scripted_run() {
    let mut tools = ToolRegistry::new();
    let scope = ScopeContract::new();
    tools.register(DeclareScopeTool::new(scope));

    let responses = vec![
        Ok(Response {
            text: Some("Declaring scope".to_string()),
            tool_calls: vec![ToolCall {
                id: "call_scope".to_string(),
                thought_signature: None,
                name: "declare_scope".to_string(),
                arguments: serde_json::json!({
                    "files": ["src/app.rs"],
                    "reason": "build app"
                })
                .to_string(),
            }],
            usage: Usage {
                input_tokens: 120,
                output_tokens: 35,
            },
        }),
        Ok(Response {
            text: Some("Task complete".to_string()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 180,
                output_tokens: 45,
            },
        }),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits {
        context_window: Some(4000),
        ..LoopLimits::default()
    };

    let result = run_agent_loop(
        Profile::Build,
        &model,
        &tools,
        "Build the application",
        &limits,
    )
    .expect("agent loop should succeed");

    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert_eq!(result.steps, 2);
    assert_eq!(result.total_input_tokens, 300);
    assert_eq!(result.total_output_tokens, 80);

    let summary = result.summary.expect("summary should be present");
    assert_eq!(summary.steps, 2);
    assert_eq!(summary.tokens_in, 300);
    assert_eq!(summary.tokens_out, 80);

    let summary_line = summary.format_line();
    assert_eq!(
        summary_line,
        "steps: 2, tokens in: 300, tokens out: 80, files edited: 0, tests changed: 0, files changed by commands: 0, outside scope: 0, dependencies added: 0"
    );
}
