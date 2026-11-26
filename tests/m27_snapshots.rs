//! Rollback snapshots in `.devcode/`: commands may not delete them, and a missing one does not
//! break the run.
use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, ModelError, ProgressEvent, ProgressSink,
    Response, ScriptedModel, ToolCall, Usage,
};
use devcode::approval::{check_shell_command, ApprovalPrompt, CommandCheck};
use devcode::config::Config;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::{self, Cursor};
use std::sync::Arc;

#[test]
fn rm_of_the_snapshot_store_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    for cmd in [
        "rm -rf .devcode",
        "rm -rf basic.py .devcode/blobs",
        "rm -rf .*",
        "rm -rf ./.devcode/",
        "cd src && rm -rf .",
    ] {
        assert!(
            matches!(
                check_shell_command(cmd, dir.path(), &[]),
                CommandCheck::Denied(_)
            ),
            "{cmd}"
        );
    }
    assert!(matches!(
        check_shell_command("rm -rf basic.py birthday.html", dir.path(), &[]),
        CommandCheck::Ask(_)
    ));
}

fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    }
}

#[test]
fn missing_snapshot_blob_does_not_fail_the_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), "old").unwrap();
    let tools = registry_with_context(MutationContext::new(
        dir.path().to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(b"y\n".to_vec()), io::sink(), true, true),
    ));
    let blobs = devcode::state::dir(dir.path()).join("blobs");
    let limits = LoopLimits {
        // Simulate a command that wiped the snapshot store right after the edit.
        progress: Some(ProgressSink(Arc::new(move |e| {
            if matches!(e, ProgressEvent::ToolResult { ref name, .. } if name == "write_file") {
                let _ = std::fs::remove_dir_all(&blobs);
            }
        }))),
        ..Default::default()
    };
    let response = |text: &str, calls: Vec<ToolCall>| -> Result<Response, ModelError> {
        Ok(Response {
            text: Some(text.into()),
            tool_calls: calls,
            usage: Usage::default(),
        })
    };
    let result = run_agent_with_config(
        Profile::Build,
        &ScriptedModel::new(vec![
            response(
                "editing",
                vec![
                    call("1", "declare_scope", json!({"files":["a"],"reason":"t"})),
                    call("2", "write_file", json!({"path":"a","content":"new"})),
                ],
            ),
            response("done", vec![]),
        ]),
        &tools,
        "task",
        &limits,
        dir.path(),
        &Config::default(),
    )
    .expect("a missing snapshot must not fail the run");
    assert_eq!(result.exit_reason, ExitReason::Completed);
}
