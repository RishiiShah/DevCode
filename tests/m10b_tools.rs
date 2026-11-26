use devcode::agent::{
    run_agent_loop, LoopLimits, Response, ScriptedModel, ToolCall, ToolRegistry, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::manifest::rollback;
use devcode::profile::Profile;
use devcode::tools::{registry_with_context, MutationContext};
use serde_json::json;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

fn registry(root: &Path, yes: bool) -> ToolRegistry {
    let tools = registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, yes),
    ));
    call(
        &tools,
        "declare_scope",
        json!({"files":["a", "b"], "reason":"test"}),
    )
    .unwrap();
    tools
}
fn call(tools: &ToolRegistry, name: &str, args: serde_json::Value) -> Result<String, String> {
    tools.execute(&ToolCall {
        id: "1".into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    })
}
#[test]
fn write_file_ask_rejected() {
    let dir = TempDir::new().unwrap();
    let tools = registry(dir.path(), true);
    for profile in [Profile::Ask, Profile::Plan] {
        let model = ScriptedModel::new(vec![
            Ok(Response {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "1".into(),
                    thought_signature: None,
                    name: "write_file".into(),
                    arguments: json!({"path":"a","content":"x"}).to_string(),
                }],
                usage: Usage::default(),
            }),
            Ok(Response {
                text: Some("done".into()),
                tool_calls: vec![],
                usage: Usage::default(),
            }),
        ]);
        let result =
            run_agent_loop(profile, &model, &tools, "write", &LoopLimits::default()).unwrap();
        assert!(result
            .messages
            .iter()
            .any(|m| m.content.contains("disabled")));
        assert!(!dir.path().join("a").exists());
    }
}
#[test]
fn write_file_boundaries() {
    let dir = TempDir::new().unwrap();
    let tools = registry(dir.path(), true);
    for path in ["../outside", ".devcode/secret"] {
        assert!(call(&tools, "write_file", json!({"path":path,"content":"x"})).is_err());
    }
}
#[test]
fn write_file_rollback() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), b"old").unwrap();
    fs::set_permissions(root.join("a"), fs::Permissions::from_mode(0o751)).unwrap();
    let tools = registry(root, true);
    call(&tools, "write_file", json!({"path":"a","content":"new"})).unwrap();
    call(&tools, "write_file", json!({"path":"b","content":"new"})).unwrap();
    rollback(root, "test", Some(1)).unwrap();
    assert_eq!(fs::read(root.join("a")).unwrap(), b"old");
    assert_eq!(
        fs::metadata(root.join("a")).unwrap().permissions().mode() & 0o777,
        0o751
    );
    assert!(!root.join("b").exists());
}
#[test]
fn write_file_prompt_rejected() {
    let dir = TempDir::new().unwrap();
    let tools = registry(dir.path(), false);
    assert!(call(&tools, "write_file", json!({"path":"a","content":"x"})).is_err());
    assert!(!dir.path().join("a").exists());
    assert!(!devcode::state::dir(dir.path())
        .join("manifests/test/1.json")
        .exists());
}
#[test]
fn edit_file_match_counts() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), "one one").unwrap();
    let tools = registry(root, true);
    for old in ["missing", "one"] {
        assert!(call(
            &tools,
            "edit_file",
            json!({"path":"a","old_string":old,"new_string":"x"})
        )
        .is_err());
    }
    call(
        &tools,
        "edit_file",
        json!({"path":"a","old_string":"one one","new_string":"x"}),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "x");
}
#[test]
fn capture_failure_refuses_write() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), "old").unwrap();
    fs::create_dir_all(devcode::state::dir(root).join("blobs")).unwrap();
    fs::set_permissions(
        devcode::state::dir(root).join("blobs"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    let tools = registry(root, true);
    assert!(call(&tools, "write_file", json!({"path":"a","content":"new"})).is_err());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "old");
    fs::set_permissions(
        devcode::state::dir(root).join("blobs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}
#[test]
fn command_reconcile_rollback() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), "old").unwrap();
    let tools = registry(root, true);
    call(
        &tools,
        "run_command",
        json!({"command":"python3 -c 'open(\"a\",\"w\").write(\"new\")'"}),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new");
    rollback(root, "test", None).unwrap();
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "old");
}
#[test]
fn command_destructive_no_tty_denied_even_yes() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("x"), "x").unwrap();
    let tools = registry(dir.path(), true);
    assert!(call(&tools, "run_command", json!({"command":"rm -rf x"})).is_err());
    assert!(dir.path().join("x").exists());
}
#[test]
fn command_redirect_denied() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.rs"), "old").unwrap();
    let tools = registry(dir.path(), true);
    let err = call(
        &tools,
        "run_command",
        json!({"command":"echo hi > src/a.rs"}),
    )
    .unwrap_err();
    assert!(err.contains("file tools"));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.rs")).unwrap(),
        "old"
    );
}
#[test]
fn schema_order_deterministic() {
    let dir = TempDir::new().unwrap();
    let names = || {
        registry(dir.path(), true)
            .schemas()
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    };
    let first = names();
    for _ in 0..20 {
        assert_eq!(first, names());
    }
}
#[test]
fn invalid_arguments_are_errors() {
    let dir = TempDir::new().unwrap();
    let tools = registry(dir.path(), true);
    for name in ["write_file", "edit_file", "run_command"] {
        assert!(call(&tools, name, json!({})).is_err());
    }
}

#[test]
fn repeated_malformed_calls_fall_back_to_text_without_bypassing_permissions() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a"), "readable marker\n").unwrap();
    let tools = registry(dir.path(), true);
    let model = ScriptedModel::new(vec![
        Err(devcode::agent::ModelError::FormatError(
            "invalid tool arguments JSON".into(),
        )),
        Err(devcode::agent::ModelError::FormatError(
            "invalid tool arguments JSON".into(),
        )),
        Ok(Response {
            text: Some(r#"{"devcode_tool_calls":[{"name":"write_file","arguments":{"path":"a","content":"replaced"}}]}"#.into()),
            tool_calls: vec![],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some(
                r#"{"devcode_tool_calls":[{"name":"read_file","arguments":{"path":"a"}}]}"#.into(),
            ),
            tool_calls: vec![],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("done".into()),
            tool_calls: vec![],
            usage: Usage::default(),
        }),
    ]);
    let result = run_agent_loop(
        Profile::Ask,
        &model,
        &tools,
        "read a",
        &LoopLimits::default(),
    )
    .unwrap();
    assert!(result
        .messages
        .iter()
        .any(|message| message.content.contains("readable marker")));
    assert_eq!(result.token_ledger.len(), 5);
    assert_eq!(
        fs::read_to_string(dir.path().join("a")).unwrap(),
        "readable marker\n"
    );
    assert!(!result.token_ledger[2].completed);
}

#[test]
fn private_reads_never_enter_model_history() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a"), "private marker\n").unwrap();
    fs::write(dir.path().join("b"), "public marker\n").unwrap();
    std::os::unix::fs::symlink("a", dir.path().join("alias")).unwrap();
    let tools = registry(dir.path(), true);
    let config: devcode::Config = serde_json::from_value(json!({"private_paths": ["a"]})).unwrap();
    let model = ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: ["a", "alias", "b"]
                .into_iter()
                .map(|path| ToolCall {
                    id: path.into(),
                    name: "read_file".into(),
                    arguments: json!({"path":path}).to_string(),
                    thought_signature: None,
                })
                .collect(),
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("done".into()),
            tool_calls: vec![],
            usage: Usage::default(),
        }),
    ]);
    let result = devcode::agent::run_agent_with_config(
        Profile::Ask,
        &model,
        &tools,
        "read a and b",
        &LoopLimits::default(),
        dir.path(),
        &config,
    )
    .unwrap();
    assert!(!result
        .messages
        .iter()
        .any(|message| message.content.contains("private marker")));
    assert!(result
        .messages
        .iter()
        .any(|message| message.content.contains("public marker")));
    assert!(!result.tool_outcomes["a"].ok);
    assert!(!result.tool_outcomes["alias"].ok);
}

#[test]
fn schema_rejection_uses_simpler_native_tools_on_the_next_attempt() {
    let dir = TempDir::new().unwrap();
    let tools = registry(dir.path(), true);
    let model = ScriptedModel::new(vec![
        Err(devcode::agent::ModelError::Network(
            r#"HTTP 400: {"error":{"param":"tools[0].function.parameters","message":"unsupported"}}"#.into(),
        )),
        Ok(Response { text: Some("done".into()), tool_calls: vec![], usage: Usage::default() }),
    ]);
    let preview = std::sync::Arc::new(std::sync::Mutex::new(None));
    let limits = LoopLimits {
        outbound_preview: Some(preview.clone()),
        ..Default::default()
    };
    let result = run_agent_loop(Profile::Ask, &model, &tools, "explain", &limits).unwrap();
    assert_eq!(result.exit_reason, devcode::agent::ExitReason::Completed);
    assert_eq!(result.token_ledger.len(), 2);
    assert!(matches!(
        result.token_ledger[0].events[0],
        devcode::agent::token_ledger::LedgerEvent::ToolDowngrade {
            mode: devcode::agent::tool_protocol::ToolMode::Simplified
        }
    ));
    let preview = preview.lock().unwrap();
    let request = preview.as_ref().unwrap();
    let read_tool = request
        .tools
        .iter()
        .find(|tool| tool.name == "read_file")
        .unwrap();
    assert_eq!(read_tool.parameters["required"], json!(["path"]));
    assert!(read_tool.parameters["properties"]["path"]
        .get("description")
        .is_none());
}

#[test]
fn provider_credential_is_redacted_before_send_and_preview() {
    struct Model;
    impl devcode::agent::Model for Model {
        fn redact_sensitive(&self, text: &str) -> String {
            text.replace("fixture-key", "[REDACTED]")
        }
        fn complete(
            &self,
            req: &devcode::agent::Request,
        ) -> Result<Response, devcode::agent::ModelError> {
            assert!(req.messages.last().unwrap().content.contains("[REDACTED]"));
            Ok(Response {
                text: Some("done".into()),
                tool_calls: vec![],
                usage: Usage::default(),
            })
        }
    }
    let preview = std::sync::Arc::new(std::sync::Mutex::new(None));
    let limits = LoopLimits {
        outbound_preview: Some(preview.clone()),
        ..Default::default()
    };
    run_agent_loop(
        Profile::Ask,
        &Model,
        &ToolRegistry::new(),
        "fixture-key",
        &limits,
    )
    .unwrap();
    let preview = preview.lock().unwrap();
    assert!(!serde_json::to_string(preview.as_ref().unwrap())
        .unwrap()
        .contains("fixture-key"));
}
