use devcode::agent::session::RateLimitedModel;
use devcode::agent::{
    ExitReason, Model, ModelError, Request, Response, ScriptedModel, ToolCall, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::{Profile, Session};
use serde_json::json;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn response(calls: Vec<ToolCall>) -> Result<Response, ModelError> {
    Ok(Response {
        text: Some("done".into()),
        tool_calls: calls,
        usage: Usage {
            input_tokens: 2,
            output_tokens: 1,
        },
    })
}
fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}
fn session(root: &Path, profile: Profile, responses: Vec<Result<Response, ModelError>>) -> Session {
    let mut config = Config::default();
    config.verification.test_command = Some("true".into());
    let context = MutationContext::new(
        root.to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, true),
    );
    let tools = registry_with_context(context);
    tools
        .execute(&call(
            "declare_scope",
            json!({"files":["a","src/a.rs","tests/a.rs"],"reason":"test"}),
        ))
        .unwrap();
    Session::new(
        root.to_path_buf(),
        config,
        profile,
        Box::new(ScriptedModel::new(responses)),
        tools,
    )
}
#[test]
fn prompt_run_real_counts() {
    let dir = tempfile::tempdir().unwrap();
    let run = session(
        dir.path(),
        Profile::Build,
        vec![
            response(vec![
                call("run_command", json!({"command":"true"})),
                call("write_file", json!({"path":"a","content":" "})),
            ]),
            response(vec![]),
        ],
    )
    .prompt("change")
    .unwrap();
    assert_eq!(run.exit_reason, ExitReason::Completed);
    let summary = run.summary.unwrap();
    assert_eq!(summary.files_edited, 1);
    assert_eq!(summary.steps, 2);
    assert_eq!(summary.tokens_in, 4);
}
#[test]
fn finish_blocked_test_stale() {
    let dir = tempfile::tempdir().unwrap();
    let run = session(
        dir.path(),
        Profile::Build,
        vec![
            response(vec![
                call("run_command", json!({"command":"true"})),
                call("write_file", json!({"path":"a","content":"source"})),
            ]),
            response(vec![]),
            response(vec![
                call("write_file", json!({"path":"tests/a.rs","content":"test"})),
                call("run_command", json!({"command":"true"})),
            ]),
            response(vec![]),
        ],
    )
    .prompt("change")
    .unwrap();
    assert_eq!(run.exit_reason, ExitReason::Completed);
    assert!(run
        .messages
        .iter()
        .any(|m| m.content.contains("test command is stale")));
    assert_eq!(run.summary.unwrap().files_edited, 2);
}
#[test]
fn finish_allowed_after_edits_and_tests() {
    let dir = tempfile::tempdir().unwrap();
    let run = session(
        dir.path(),
        Profile::Build,
        vec![
            response(vec![
                call("run_command", json!({"command":"true"})),
                call("write_file", json!({"path":"src/a.rs","content":"source"})),
                call("write_file", json!({"path":"tests/a.rs","content":"test"})),
                call("run_command", json!({"command":"true"})),
            ]),
            response(vec![]),
        ],
    )
    .prompt("change")
    .unwrap();
    assert_eq!(run.exit_reason, ExitReason::Completed);
    assert!(run.verification.as_ref().unwrap().contains("fresh"));
    assert_eq!(run.summary.as_ref().unwrap().files_edited, 2);
    assert!(!run
        .messages
        .iter()
        .any(|m| m.content.contains("Verification blocked")));
}
#[test]
fn ask_profile_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let run = session(
        dir.path(),
        Profile::Ask,
        vec![
            response(vec![call("write_file", json!({"path":"a","content":"x"}))]),
            response(vec![]),
        ],
    )
    .prompt("change")
    .unwrap();
    assert!(!dir.path().join("a").exists());
    assert!(run.messages.iter().any(|m| m.content.contains("disabled")));
    assert_eq!(run.summary.unwrap().files_edited, 0);
}
#[test]
fn missing_key_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".devcode.json"),r#"{"provider":{"kind":"openai_compat","base_url":"http://localhost","api_key_env":"M10C_TEST_MISSING_KEY","model":"x","context_window":1000}}"#).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(dir.path())
        .env_remove("M10C_TEST_MISSING_KEY")
        .arg("hello")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("M10C_TEST_MISSING_KEY"));
}
#[test]
fn rollback_no_manifest_message() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(dir.path())
        .arg("rollback")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no manifest found"));
}
#[test]
fn max_rpm_request_spacing() {
    let model = ScriptedModel::new(vec![response(vec![]), response(vec![])]);
    let limited = RateLimitedModel::new(Box::new(model), Duration::from_millis(50));
    let request = Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
    };
    limited.complete(&request).unwrap();
    let start = Instant::now();
    limited.complete(&request).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(45));
}
#[test]
fn rate_limit_exit_reason() {
    let dir = tempfile::tempdir().unwrap();
    let run = session(dir.path(), Profile::Ask, vec![Err(ModelError::RateLimited)])
        .prompt("x")
        .unwrap();
    assert_eq!(run.exit_reason, ExitReason::RateLimited);
    assert_eq!(run.steps, 0);
}
