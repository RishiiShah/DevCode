use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, ModelError, ProgressEvent, ProgressSink,
    Response, ScriptedModel, ToolCall, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::{self, Cursor};
use std::sync::{Arc, Mutex};

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    }
}
fn response(text: &str, calls: Vec<ToolCall>) -> Result<Response, ModelError> {
    Ok(Response {
        text: Some(text.into()),
        tool_calls: calls,
        usage: Usage::default(),
    })
}

#[test]
fn no_test_command_finishes_unverified_and_progress_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry_with_context(MutationContext::new(
        dir.path().to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(b"y\n".to_vec()), io::sink(), true, true),
    ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let limits = LoopLimits {
        progress: Some(ProgressSink(Arc::new(move |e| {
            sink.lock().unwrap().push(e)
        }))),
        ..Default::default()
    };
    let result = run_agent_with_config(
        Profile::Build,
        &ScriptedModel::new(vec![
            response(
                "editing",
                vec![
                    call("declare_scope", json!({"files":["a"],"reason":"t"})),
                    call("write_file", json!({"path":"a","content":"x"})),
                ],
            ),
            response("all done", vec![]),
        ]),
        &tools,
        "task",
        &limits,
        dir.path(),
        &Config::default(),
    )
    .unwrap();
    // The gate cannot be satisfied without a test command; it must not loop to the step limit.
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert_eq!(result.steps, 2);
    assert!(result.verification.unwrap().contains("UNVERIFIED"));
    let events = events.lock().unwrap();
    assert!(events.contains(&ProgressEvent::Thinking { step: 1 }));
    assert!(events.contains(&ProgressEvent::Assistant {
        text: "editing".into()
    }));
    assert!(events.iter().any(
        |e| matches!(e, ProgressEvent::ToolCall { name, summary } if name == "write_file" && summary == "a")
    ));
    assert!(events.iter().any(
        |e| matches!(e, ProgressEvent::ToolResult { name, ok: true, .. } if name == "write_file")
    ));
    // The user sees the diff of the write; the model sees only "Updated a".
    assert!(events.iter().any(|e| matches!(
        e,
        ProgressEvent::ToolResult { diff: Some(d), preview, .. } if d.contains("+x") && preview == "Updated a"
    )));
}
