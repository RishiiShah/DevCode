use clap::Parser;
use devcode::agent::{
    run_agent_with_config, LoopLimits, Response, ScriptedModel, ToolCall, ToolRegistry, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::{Config, Profile};
use serde_json::{json, Value};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    }
}
fn execute(tools: &ToolRegistry, name: &str, args: Value) {
    tools.execute(&call(name, args)).unwrap();
}
fn setup(root: &Path) -> ToolRegistry {
    let tools = registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "session".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, true),
    ));
    execute(
        &tools,
        "declare_scope",
        json!({"files":["a","b"],"reason":"test"}),
    );
    tools
}
fn three_steps(root: &Path) {
    fs::write(root.join("a"), "old a").unwrap();
    fs::write(root.join("b"), "old b").unwrap();
    let tools = setup(root);
    execute(&tools, "write_file", json!({"path":"a","content":"new a"}));
    execute(
        &tools,
        "run_command",
        json!({"command":"python3 -c 'open(\"created\",\"w\").write(\"x\")'"}),
    );
    execute(&tools, "write_file", json!({"path":"b","content":"new b"}));
}
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn default_rolls_back_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    three_steps(root);
    let out = cli(root, &["rollback"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    for path in ["a", "b", "created"] {
        assert!(text.contains(path));
    }
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "old a");
    assert_eq!(fs::read_to_string(root.join("b")).unwrap(), "old b");
    assert!(!root.join("created").exists());
}
#[test]
fn rollback_from_step_two() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    three_steps(root);
    let out = cli(root, &["rollback", "--step", "2"]);
    assert!(out.status.success());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new a");
    assert_eq!(fs::read_to_string(root.join("b")).unwrap(), "old b");
    assert!(!root.join("created").exists());
}
#[test]
fn rollback_conflict_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    three_steps(root);
    fs::write(root.join("b"), "external").unwrap();
    let out = cli(root, &["rollback"]);
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "new a");
    assert_eq!(fs::read_to_string(root.join("b")).unwrap(), "external");
    assert!(root.join("created").exists());
}
#[test]
fn rollback_repeated_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a"), "first").unwrap();
    let tools = setup(root);
    execute(&tools, "write_file", json!({"path":"a","content":"second"}));
    execute(&tools, "write_file", json!({"path":"a","content":"third"}));
    assert!(cli(root, &["rollback"]).status.success());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "first");
}
fn response(calls: Vec<ToolCall>) -> Result<Response, devcode::agent::ModelError> {
    Ok(Response {
        text: Some("secret-response".into()),
        tool_calls: calls,
        usage: Usage {
            input_tokens: Some(7),
            output_tokens: Some(3),
            ..Usage::default()
        },
    })
}
#[test]
fn summary_counts_trace_and_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(Command::new("git")
        .current_dir(root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    fs::write(root.join("a"), "x").unwrap();
    let tools = setup(root);
    let model = ScriptedModel::new(vec![
        response(vec![
            call(
                "edit_file",
                json!({"path":"a","old_string":"x","new_string":"x "}),
            ),
            call(
                "edit_file",
                json!({"path":"a","old_string":"x ","new_string":"x  "}),
            ),
            call(
                "run_command",
                json!({"command":"python3 -c 'open(\"generated\",\"w\").write(\" \")'"}),
            ),
        ]),
        response(vec![]),
    ]);
    let limits = LoopLimits {
        trace: true,
        ..Default::default()
    };
    let result = run_agent_with_config(
        Profile::Build,
        &model,
        &tools,
        "secret-prompt",
        &limits,
        root,
        &Config::default(),
    )
    .unwrap();
    let summary = result.summary.unwrap();
    assert_eq!(summary.files_edited, 1);
    assert_eq!(summary.files_changed_by_commands, 1);
    assert!(summary
        .format_line()
        .contains("files changed by commands: 1"));
    let out = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(!String::from_utf8(out.stdout).unwrap().contains(".devcode"));
    let trace = fs::read_to_string(devcode::state::dir(root).join("trace.jsonl")).unwrap();
    assert!(!trace.contains("secret"));
    assert!(!trace.contains("python3"));
    let lines: Vec<Value> = trace
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    for (i, v) in lines.iter().enumerate() {
        assert_eq!(v["step"], i + 1);
        for key in [
            "tool_name",
            "argument_bytes",
            "output_bytes",
            "input_tokens",
            "output_tokens",
            "model_ms",
            "tool_ms",
            "approval_wait_ms",
        ] {
            assert!(v.get(key).is_some());
        }
        assert_eq!(v["input_tokens"], 7);
        assert_eq!(v["output_tokens"], 3);
    }
    assert!(
        devcode::Cli::try_parse_from(["devcode", "--trace", "hello"])
            .unwrap()
            .trace
    );
}
