use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, Model, ModelError, Request, Response,
    ScriptedModel, ToolCall, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

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
        text: Some("done".into()),
        tool_calls: calls,
        usage: Usage::default(),
    })
}
fn tools(root: &std::path::Path) -> devcode::agent::ToolRegistry {
    registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, true),
    ))
}
fn config() -> Config {
    let mut c = Config::default();
    c.verification.test_command = Some("true".into());
    c
}
fn scope() -> ToolCall {
    call(
        "declare_scope",
        json!({"files":["a","tests/a"],"reason":"test"}),
    )
}
fn write(path: &str, text: &str) -> ToolCall {
    call("write_file", json!({"path":path,"content":text}))
}
fn cmd() -> ToolCall {
    call("run_command", json!({"command":"true"}))
}
#[test]
fn flip_flop_stops_no_progress() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), "A").unwrap();
    let tools = tools(dir.path());
    let model = ScriptedModel::new(vec![response(vec![
        cmd(),
        scope(),
        write("a", "B"),
        write("a", "A"),
        write("a", "B"),
        write("a", "A"),
    ])]);
    let result = run_agent_with_config(
        Profile::Build,
        &model,
        &tools,
        "task",
        &LoopLimits::default(),
        dir.path(),
        &config(),
    )
    .unwrap();
    assert_eq!(result.exit_reason, ExitReason::NoProgress);
    assert_eq!(result.no_progress, Some(("a".into(), 3)));
}
struct ChangeAfterTest {
    inner: ScriptedModel,
    file: PathBuf,
    calls: Mutex<usize>,
}
impl Model for ChangeAfterTest {
    fn complete(&self, r: &Request) -> Result<Response, ModelError> {
        let mut count = self.calls.lock().unwrap();
        *count += 1;
        if *count == 2 {
            std::fs::write(&self.file, "external").unwrap();
        }
        self.inner.complete(r)
    }
}
#[test]
fn external_change_after_test_blocks_finish() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(Command::new("git")
        .current_dir(root)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("a"), "initial").unwrap();
    assert!(Command::new("git")
        .current_dir(root)
        .args(["add", "a"])
        .status()
        .unwrap()
        .success());
    let model = ChangeAfterTest {
        inner: ScriptedModel::new(vec![
            response(vec![
                cmd(),
                scope(),
                write("a", "changed"),
                write("tests/a", "test"),
                cmd(),
            ]),
            response(vec![]),
            response(vec![cmd()]),
            response(vec![]),
        ]),
        file: root.join("a"),
        calls: Mutex::new(0),
    };
    let result = run_agent_with_config(
        Profile::Build,
        &model,
        &tools(root),
        "task",
        &LoopLimits::default(),
        root,
        &config(),
    )
    .unwrap();
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert!(result.messages.iter().any(|m| m
        .content
        .contains("Workspace changed since the test command ran")));
}
#[test]
fn unchanged_after_test_allows_finish() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let model = ScriptedModel::new(vec![
        response(vec![
            cmd(),
            scope(),
            write("a", "changed"),
            write("tests/a", "test"),
            cmd(),
        ]),
        response(vec![]),
    ]);
    let result = run_agent_with_config(
        Profile::Build,
        &model,
        &tools(root),
        "task",
        &LoopLimits::default(),
        root,
        &config(),
    )
    .unwrap();
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert!(!result
        .messages
        .iter()
        .any(|m| m.content.contains("Workspace changed")));
}
#[test]
fn blocked_observation_explains_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let model = ScriptedModel::new(vec![
        response(vec![cmd(), scope(), write("a", "changed"), cmd()]),
        response(vec![]),
        response(vec![write("tests/a", "test"), cmd()]),
        response(vec![]),
    ]);
    let result = run_agent_with_config(
        Profile::Build,
        &model,
        &tools(root),
        "task",
        &LoopLimits::default(),
        root,
        &config(),
    )
    .unwrap();
    assert!(result.messages.iter().any(|m| m.content.contains(
        "Either add or update a test, or run the test command at a point where it fails"
    )));
}
