use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, ModelError, Response, ScriptedModel, ToolCall,
    Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::{self, Cursor, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
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
fn scope() -> ToolCall {
    call("declare_scope", json!({"files":["a"],"reason":"test"}))
}
fn write() -> ToolCall {
    call("write_file", json!({"path":"a","content":"fixed"}))
}
fn cmd(command: &str) -> ToolCall {
    call("run_command", json!({"command":command}))
}
fn run(
    command: Option<&str>,
    script: Vec<Result<Response, ModelError>>,
    output: Output,
    auto: bool,
) -> (tempfile::TempDir, devcode::agent::LoopResult) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.verification.test_command = command.map(str::to_owned);
    let tools = registry_with_context(MutationContext::new(
        dir.path().to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(b"y\n".to_vec()), output, true, auto),
    ));
    let result = run_agent_with_config(
        Profile::Build,
        &ScriptedModel::new(script),
        &tools,
        "task",
        &LoopLimits::default(),
        dir.path(),
        &cfg,
    )
    .unwrap();
    (dir, result)
}
#[test]
fn edit_before_test_refused() {
    let (dir, res) = run(
        Some("true"),
        vec![response(vec![scope(), write()]), response(vec![])],
        Output::default(),
        true,
    );
    assert!(!dir.path().join("a").exists());
    assert!(res.messages.iter().any(|m|m.content=="Run the configured test command first: true. This records whether the problem reproduces."));
}
#[test]
fn baseline_then_edit_succeeds() {
    let (dir, res) = run(
        Some("test -f a"),
        vec![
            response(vec![scope(), cmd("test -f a"), write(), cmd("test -f a")]),
            response(vec![]),
        ],
        Output::default(),
        true,
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a")).unwrap(),
        "fixed"
    );
    assert_eq!(res.exit_reason, ExitReason::Completed);
}
#[test]
fn unconfigured_test_allows_edit() {
    let (dir, res) = run(
        None,
        vec![
            response(vec![
                scope(),
                call("write_file", json!({"path":"a","content":" "})),
            ]),
            response(vec![]),
        ],
        Output::default(),
        true,
    );
    assert!(dir.path().join("a").exists());
    assert_eq!(res.exit_reason, ExitReason::Completed);
}
#[test]
fn refused_edit_never_prompts() {
    let out = Output::default();
    let (dir, _) = run(
        Some("true"),
        vec![response(vec![scope(), write()]), response(vec![])],
        out.clone(),
        false,
    );
    assert!(!dir.path().join("a").exists());
    assert!(out.0.lock().unwrap().is_empty());
}
#[test]
fn failing_baseline_one_edit_green() {
    let (dir, res) = run(
        Some("test -f a"),
        vec![
            response(vec![scope(), cmd("test -f a"), write(), cmd("test -f a")]),
            response(vec![]),
        ],
        Output::default(),
        true,
    );
    assert_eq!(res.exit_reason, ExitReason::Completed);
    assert_eq!(res.summary.unwrap().files_edited, 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a")).unwrap(),
        "fixed"
    );
    assert!(res.verification.unwrap().contains("fresh"));
}

#[test]
fn edit_file_before_test_is_refused() {
    let output = Output::default();
    let (dir, result) = run(
        Some("true"),
        vec![
            response(vec![
                scope(),
                call(
                    "edit_file",
                    json!({"path":"a", "old_string":"x", "new_string":"y"}),
                ),
            ]),
            response(vec![]),
        ],
        output.clone(),
        false,
    );
    assert!(!dir.path().join("a").exists());
    assert!(result.messages.iter().any(|m| m
        .content
        .contains("Run the configured test command first: true.")));
    assert!(output.0.lock().unwrap().is_empty());
}
