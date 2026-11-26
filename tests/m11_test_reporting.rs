use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, ModelError, Response, ScriptedModel, ToolCall,
    Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::Cursor;
use std::path::Path;

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    }
}
fn response(calls: Vec<ToolCall>) -> Result<Response, ModelError> {
    Ok(Response {
        text: Some("untrusted model text".into()),
        tool_calls: calls,
        usage: Usage::default(),
    })
}
fn scope(paths: &[&str]) -> ToolCall {
    call("declare_scope", json!({"files":paths,"reason":"test"}))
}
fn write(path: &str, content: &str) -> ToolCall {
    call("write_file", json!({"path":path,"content":content}))
}
fn command(cmd: &str) -> ToolCall {
    call("run_command", json!({"command":cmd}))
}
fn run(root: &Path, config: Config, calls: Vec<ToolCall>) -> devcode::agent::LoopResult {
    let tools = registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, true),
    ));
    let model = ScriptedModel::new(vec![response(calls), response(vec![])]);
    run_agent_with_config(
        Profile::Build,
        &model,
        &tools,
        "task",
        &LoopLimits::default(),
        root,
        &config,
    )
    .unwrap()
}
#[test]
fn source_and_test_edits_report_lines() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/test_a.py"), "one\nkeep\n").unwrap();
    let mut config = Config::default();
    config.verification.test_command = Some("true".into());
    let result = run(
        root,
        config,
        vec![
            command("true"),
            scope(&["src/a", "tests/test_a.py"]),
            write("src/a", "source\n"),
            write("tests/test_a.py", "two\nkeep\nnew\n"),
            command("true"),
        ],
    );
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert!(result.verification.unwrap().contains("Tests changed by the agent: 1 file(s), +2/-1 lines: tests/test_a.py. Review them; passing tests here were written in the same run."));
    assert_eq!(result.summary.unwrap().tests_changed, 1);
}
#[test]
fn source_only_red_green_reports_existing_tests() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.verification.test_command = Some("test -f a".into());
    let result = run(
        dir.path(),
        config,
        vec![
            command("test -f a"),
            scope(&["a"]),
            write("a", "fixed"),
            command("test -f a"),
        ],
    );
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert!(result.verification.unwrap().contains("No tests changed by the agent. Gate satisfied by a failing-then-passing run of the existing tests."));
    assert_eq!(result.summary.unwrap().tests_changed, 0);
}
#[test]
fn no_test_command_no_notice() {
    let dir = tempfile::tempdir().unwrap();
    let result = run(
        dir.path(),
        Config::default(),
        vec![scope(&["a"]), write("a", " ")],
    );
    let summary = result.verification.unwrap();
    assert!(!summary.contains("Tests changed by the agent"));
    assert!(!summary.contains("No tests changed by the agent"));
    assert_eq!(result.summary.unwrap().tests_changed, 0);
}
#[test]
fn test_paths_sorted_with_custom_glob() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.verification.test_command = Some("true".into());
    config.verification.test_globs = vec!["checks/**".into()];
    let result = run(
        dir.path(),
        config,
        vec![
            command("true"),
            scope(&["checks/z", "checks/a"]),
            write("checks/z", "z\n"),
            write("checks/a", "a\n"),
            command("true"),
        ],
    );
    assert!(result
        .verification
        .unwrap()
        .contains("2 file(s), +2/-0 lines: checks/a, checks/z."));
    assert_eq!(result.summary.unwrap().tests_changed, 2);
}
