//! Saved conversations: every prompt saves the conversation, and resume continues it.
use clap::Parser;
use devcode::agent::{Message, ModelError, Response, Role, ScriptedModel, Usage};
use devcode::config::Config;
use devcode::{Cli, Profile, Session};

fn reply(text: &str) -> Result<Response, ModelError> {
    Ok(Response {
        text: Some(text.into()),
        tool_calls: Vec::new(),
        usage: Usage::default(),
    })
}

fn session(root: &std::path::Path, replies: Vec<Result<Response, ModelError>>) -> Session {
    Session::new(
        root.to_path_buf(),
        Config::default(),
        Profile::Ask,
        Box::new(ScriptedModel::new(replies)),
        devcode::tools::registry(root.to_path_buf(), false),
    )
}

#[test]
fn a_new_session_resumes_the_saved_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = session(dir.path(), vec![reply("hello")]);
    first.prompt("first question\nmore detail").unwrap();

    let list = devcode::conversation::list(dir.path());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].title, "first question");
    assert_eq!(list[0].turns, 1);

    // Empty id takes the latest, as `devcode --continue` does.
    let mut second = session(dir.path(), vec![reply("again")]);
    let messages = second.resume("").unwrap();
    assert!(messages
        .iter()
        .any(|m| m.role == Role::User && m.content.contains("first question")));
    let run = second.prompt("follow up").unwrap();
    let users: Vec<&Message> = run
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .collect();
    assert!(
        users[0].content.contains("first question"),
        "history carried over"
    );
    assert!(users.last().unwrap().content.contains("follow up"));

    // Still one conversation, now with two prompts; /clear starts a second one.
    let list = devcode::conversation::list(dir.path());
    assert_eq!((list.len(), list[0].turns), (1, 2));
    second.clear_history();
    let mut third = second;
    let _ = third.prompt("unrelated");
    assert_eq!(devcode::conversation::list(dir.path()).len(), 2);
}

#[test]
fn resume_of_an_unknown_id_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session(dir.path(), Vec::new());
    assert!(s.resume("").is_err(), "nothing saved yet");
    assert!(s.resume("../../etc").is_err());
}

#[test]
fn cli_continue_and_resume_flags() {
    let c = Cli::try_parse_from(["devcode", "-c"]).unwrap();
    assert!(c.continue_last);
    let r = Cli::try_parse_from(["devcode", "--resume"]).unwrap();
    assert_eq!(r.resume.as_deref(), Some(""));
    let r = Cli::try_parse_from(["devcode", "--resume", "abc"]).unwrap();
    assert_eq!(r.resume.as_deref(), Some("abc"));
}

#[test]
fn resume_round_trips_edit_diff_ok_flag_and_tokens_and_loads_legacy_json() {
    use devcode::agent::ToolCall;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "before\n").unwrap();

    let model = ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: vec![
                ToolCall {
                    id: "c1".into(),
                    thought_signature: None,
                    name: "declare_scope".into(),
                    arguments: json!({"files": ["a.txt"], "reason": "test"}).to_string(),
                },
                ToolCall {
                    id: "c2".into(),
                    thought_signature: None,
                    name: "edit_file".into(),
                    arguments: json!({
                        "path": "a.txt",
                        "old_string": "before",
                        "new_string": "after"
                    })
                    .to_string(),
                },
                ToolCall {
                    id: "c3".into(),
                    thought_signature: None,
                    name: "read_file".into(),
                    arguments: json!({"path": "missing.txt"}).to_string(),
                },
            ],
            usage: Usage {
                input_tokens: Some(150),
                output_tokens: Some(40),
                ..Usage::default()
            },
        }),
        Ok(Response {
            text: Some("edited".into()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: Some(210),
                output_tokens: Some(15),
                cache_read_tokens: Some(63),
                ..Usage::default()
            },
        }),
    ]);

    let mut first = Session::new(
        dir.path().to_path_buf(),
        Config::default(),
        Profile::Build,
        Box::new(model),
        devcode::tools::registry(dir.path().to_path_buf(), true),
    );
    first.prompt("change a.txt").unwrap();

    // Verify 0600/0700 permissions on saved transcript and directory.
    let conv_dir = devcode::state::dir(dir.path()).join("conversations");
    let dir_mode = std::fs::metadata(&conv_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    let summary = &devcode::conversation::list(dir.path())[0];
    let file_mode = std::fs::metadata(conv_dir.join(format!("{}.json", summary.id)))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);

    // Resuming restores per-tool ok flag, unified diff, and token totals.
    let mut second = session(dir.path(), Vec::new());
    let loaded = second.resume_full(&summary.id).unwrap();
    assert_eq!(loaded.token_ledger.len(), 2);
    assert_eq!(loaded.token_ledger[0].reported.input_tokens, Some(150));
    assert!(loaded.token_ledger[0].reported.cache_read_tokens.is_none());
    assert!(loaded.token_ledger[0].estimated.system_prompt > 0);
    assert!(loaded.token_ledger[0].events.iter().any(|event| matches!(
        event,
        devcode::agent::token_ledger::LedgerEvent::Checkpoint { .. }
    )));
    assert!(loaded.token_ledger[1].events.iter().any(|event| matches!(
        event,
        devcode::agent::token_ledger::LedgerEvent::Gate { .. }
    )));
    assert!(loaded.token_ledger[1].estimated.file_reads > 0);
    assert!(second
        .token_status()
        .unwrap()
        .contains("cache hit rate 30.0%"));
    assert_eq!(loaded.total_in, 360);
    assert_eq!(loaded.total_out, 55);
    assert_eq!(loaded.context_used, 210);
    let edit_outcome = loaded.tool_outcomes.get("c2").expect("edit outcome saved");
    assert!(edit_outcome.ok);
    let diff = edit_outcome.diff.as_deref().expect("edit diff saved");
    assert!(
        diff.contains("-before") && diff.contains("+after"),
        "{diff}"
    );
    let err_outcome = loaded.tool_outcomes.get("c3").expect("failed read saved");
    assert!(!err_outcome.ok);
    assert!(err_outcome.diff.is_none());

    // Old-format JSON (without tool_outcomes / token fields) still loads cleanly.
    let legacy_id = "legacy123";
    std::fs::write(
        conv_dir.join(format!("{legacy_id}.json")),
        serde_json::json!({
            "id": legacy_id,
            "updated": 1700000000u64,
            "title": "old prompt",
            "messages": [
                {"role": "user", "content": "old prompt"},
                {"role": "assistant", "content": "old reply"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let legacy = devcode::conversation::load_full(dir.path(), legacy_id).unwrap();
    assert_eq!(legacy.title, "old prompt");
    assert!(legacy.tool_outcomes.is_empty());
    assert!(legacy.token_ledger.is_empty());
    assert_eq!(
        (legacy.total_in, legacy.total_out, legacy.context_used),
        (0, 0, 0)
    );
}

#[test]
fn session_compact_and_export_markdown() {
    use devcode::agent::ToolCall;
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "one\n").unwrap();

    let model = ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: vec![
                ToolCall {
                    id: "t1".into(),
                    thought_signature: None,
                    name: "declare_scope".into(),
                    arguments: json!({"files": ["note.txt"], "reason": "update"}).to_string(),
                },
                ToolCall {
                    id: "t2".into(),
                    thought_signature: None,
                    name: "edit_file".into(),
                    arguments: json!({
                        "path": "note.txt",
                        "old_string": "one",
                        "new_string": "two"
                    })
                    .to_string(),
                },
            ],
            usage: Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..Usage::default()
            },
        }),
        Ok(Response {
            text: Some("updated note".into()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: Some(120),
                output_tokens: Some(10),
                ..Usage::default()
            },
        }),
        Ok(Response {
            text: Some("second answer".into()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: Some(140),
                output_tokens: Some(12),
                ..Usage::default()
            },
        }),
    ]);

    let mut s = Session::new(
        dir.path().to_path_buf(),
        Config::default(),
        Profile::Build,
        Box::new(model),
        devcode::tools::registry(dir.path().to_path_buf(), true),
    );

    assert!(s.export_markdown(std::path::Path::new("empty.md")).is_err());
    assert_eq!(s.compact().unwrap(), "nothing to compact yet");

    s.prompt("first turn").unwrap();
    s.prompt("second turn").unwrap();

    let exported = s
        .export_markdown(std::path::Path::new("out/transcript.md"))
        .unwrap();
    let md = std::fs::read_to_string(&exported).unwrap();
    assert!(md.contains("# first turn"));
    assert!(md.contains("## User"));
    assert!(md.contains("## Assistant"));
    assert!(md.contains("Tool `edit_file`"));
    assert!(md.contains("```diff"));
    assert!(md.contains("-one") && md.contains("+two"));
    assert!(md.contains("second answer"));
    assert!(s.export_markdown(&exported).is_err());
    assert_eq!(std::fs::read_to_string(&exported).unwrap(), md);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&exported).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let summary = s.compact().unwrap();
    assert!(summary.contains("Compacted conversation"), "{summary}");
    let summary_id = devcode::conversation::list(dir.path())[0].id.clone();
    let reloaded = devcode::conversation::load_full(dir.path(), &summary_id).unwrap();
    assert!(reloaded.messages.iter().any(|m| m
        .content
        .contains("HARNESS DETERMINISTIC COMPACTION SUMMARY")));
}

#[test]
fn resume_restores_declared_scope_for_next_edit() {
    use devcode::agent::ToolCall;
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hero.tsx"), "old\n").unwrap();

    let first_model = ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "s1".into(),
                thought_signature: None,
                name: "declare_scope".into(),
                arguments: json!({"files": ["hero.tsx"], "reason": "style"}).to_string(),
            }],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("declared".into()),
            tool_calls: Vec::new(),
            usage: Usage::default(),
        }),
        reply("ready"),
    ]);
    let mut first = Session::new(
        dir.path().to_path_buf(),
        Config::default(),
        Profile::Build,
        Box::new(first_model),
        devcode::tools::registry(dir.path().to_path_buf(), true),
    );
    first.prompt("prepare").unwrap();

    first.prompt("prepare another turn").unwrap();
    first.compact().unwrap();

    // New session resumes and edits hero.tsx directly without re-calling declare_scope.
    let second_model = ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "e1".into(),
                thought_signature: None,
                name: "edit_file".into(),
                arguments: json!({
                    "path": "hero.tsx",
                    "old_string": "old",
                    "new_string": "new"
                })
                .to_string(),
            }],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("updated".into()),
            tool_calls: Vec::new(),
            usage: Usage::default(),
        }),
    ]);
    let mut second = Session::new(
        dir.path().to_path_buf(),
        Config::default(),
        Profile::Build,
        Box::new(second_model),
        devcode::tools::registry(dir.path().to_path_buf(), true),
    );
    let loaded = second.resume("").unwrap();
    assert!(loaded.iter().all(|m| m.tool_calls.is_empty()));
    let run = second.prompt("continue").unwrap();
    assert_eq!(run.exit_reason, devcode::agent::ExitReason::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("hero.tsx")).unwrap(),
        "new\n"
    );
}

#[test]
fn failed_model_call_keeps_reported_zero_and_missing_usage() {
    let dir = tempfile::tempdir().unwrap();
    let mut original = session(
        dir.path(),
        vec![Err(ModelError::WithUsage {
            error: Box::new(ModelError::Other("failed after reporting usage".into())),
            usage: Box::new(Usage {
                input_tokens: Some(0),
                ..Usage::default()
            }),
        })],
    );
    assert!(original.prompt("run task").is_err());

    let mut resumed = session(dir.path(), Vec::new());
    let loaded = resumed.resume_full("").unwrap();
    assert_eq!(loaded.token_ledger.len(), 1);
    let row = &loaded.token_ledger[0];
    assert!(!row.completed);
    assert_eq!(row.reported.input_tokens, Some(0));
    assert_eq!(row.reported.output_tokens, None);
    let status = resumed.token_status().unwrap();
    assert!(status.contains("0 input, missing output"), "{status}");
    assert!(status.contains("cache hit rate missing"), "{status}");
}

#[test]
fn private_policy_rejects_history_saved_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut previous = session(dir.path(), vec![reply("prior data")]);
    previous.prompt("first task").unwrap();

    let config: Config = serde_json::from_value(serde_json::json!({
        "private_paths": ["local/private.txt"]
    }))
    .unwrap();
    let mut protected = Session::new(
        dir.path().to_path_buf(),
        config,
        Profile::Ask,
        Box::new(ScriptedModel::new(vec![reply("protected reply")])),
        devcode::tools::registry(dir.path().to_path_buf(), false),
    );
    assert!(protected.resume_full("").is_err());
    protected.prompt("new task").unwrap();
    assert!(protected.outbound_preview().unwrap().contains("new task"));
    assert!(!protected.outbound_preview().unwrap().contains("prior data"));
    protected.resume_full("").unwrap();
    assert!(protected.outbound_preview().is_none());
}
