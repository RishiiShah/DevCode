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
