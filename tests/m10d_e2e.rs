use devcode::agent::session::session_state;
use devcode::agent::{
    run_agent_with_config, ExitReason, LoopLimits, Response, ScriptedModel, ToolCall, ToolRegistry,
    Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::Config;
use devcode::gate::ToolKind;
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("src/a.rs"), "base\n").unwrap();
    fs::write(root.join("dirty"), "base\n").unwrap();
    fs::write(root.join("tests/a.rs"), "old test\n").unwrap();
    git(root, &["add", "src/a.rs", "dirty", "tests/a.rs"]);
    fs::write(root.join("src/a.rs"), "partially staged\n").unwrap();
    fs::set_permissions(root.join("src/a.rs"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("dirty"), "dirty\n").unwrap();
    fs::write(root.join("untracked"), "untracked\n").unwrap();
    fs::write(root.join(".git/info/exclude"), ".devcode/\n").unwrap();
    dir
}
fn tool(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: name.into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}
fn response(calls: Vec<ToolCall>) -> Result<Response, devcode::agent::ModelError> {
    Ok(Response {
        text: Some("done".into()),
        tool_calls: calls,
        usage: Usage {
            input_tokens: 7,
            output_tokens: 3,
        },
    })
}
fn scope(files: &[&str]) -> ToolCall {
    tool("declare_scope", json!({"files":files,"reason":"test"}))
}
fn write(path: &str, content: &str) -> ToolCall {
    tool("write_file", json!({"path":path,"content":content}))
}
fn command() -> ToolCall {
    tool("run_command", json!({"command":"true"}))
}
fn tools(root: &Path, input: &str, output: Output) -> ToolRegistry {
    registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "e2e".into(),
        ApprovalPrompt::new(Cursor::new(input.as_bytes().to_vec()), output, true, true),
    ))
}
fn run(
    root: &Path,
    profile: Profile,
    tools: &ToolRegistry,
    script: Vec<Result<Response, devcode::agent::ModelError>>,
) -> devcode::agent::LoopResult {
    let mut config = Config::default();
    config.verification.test_command = Some("true".into());
    run_agent_with_config(
        profile,
        &ScriptedModel::new(script),
        tools,
        "task",
        &LoopLimits::default(),
        root,
        &config,
    )
    .unwrap()
}
#[test]
fn scoped_edit_gate_and_cli_rollback_preserve_git_state() {
    let dir = repo();
    let root = dir.path();
    let paths = ["src/a.rs", "tests/a.rs", "dirty", "untracked"];
    let before: Vec<_> = paths
        .iter()
        .map(|p| {
            (
                fs::read(root.join(p)).unwrap(),
                fs::metadata(root.join(p)).unwrap().permissions().mode(),
            )
        })
        .collect();
    let status = git(root, &["status", "--porcelain"]);
    let index = Sha256::digest(fs::read(root.join(".git/index")).unwrap());
    let tools = tools(root, "", Output::default());
    let result = run(
        root,
        Profile::Build,
        &tools,
        vec![
            response(vec![
                command(),
                scope(&["src/a.rs", "tests/a.rs"]),
                write("src/a.rs", "changed\n"),
                write("tests/a.rs", "new test\n"),
                command(),
            ]),
            response(vec![]),
        ],
    );
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert!(result.verification.unwrap().contains("fresh"));
    let rollback = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root)
        .args(["rollback", "--step", "1"])
        .output()
        .unwrap();
    assert!(
        rollback.status.success(),
        "{}",
        String::from_utf8_lossy(&rollback.stderr)
    );
    for (p, (bytes, mode)) in paths.iter().zip(before) {
        assert_eq!(fs::read(root.join(p)).unwrap(), bytes);
        assert_eq!(
            fs::metadata(root.join(p)).unwrap().permissions().mode(),
            mode
        );
    }
    assert_eq!(git(root, &["status", "--porcelain"]), status);
    assert_eq!(
        Sha256::digest(fs::read(root.join(".git/index")).unwrap()),
        index
    );
}
#[test]
fn edit_after_test_blocks_finish() {
    let dir = repo();
    let tools = tools(dir.path(), "", Output::default());
    let result = run(
        dir.path(),
        Profile::Build,
        &tools,
        vec![
            response(vec![
                command(),
                scope(&["src/a.rs", "tests/a.rs"]),
                write("src/a.rs", "changed"),
                write("tests/a.rs", "test"),
                command(),
                write("src/a.rs", "later"),
            ]),
            response(vec![]),
            response(vec![command()]),
            response(vec![]),
        ],
    );
    assert!(result.messages.iter().any(|m| m.content.contains("stale")));
    assert_eq!(result.exit_reason, ExitReason::Completed);
}
#[test]
fn outside_scope_prompts_and_counts() {
    let dir = repo();
    let output = Output::default();
    let tools = tools(dir.path(), "yes\n", output.clone());
    let result = run(
        dir.path(),
        Profile::Build,
        &tools,
        vec![
            response(vec![
                command(),
                scope(&[]),
                write("untracked", "untracked \n"),
            ]),
            response(vec![]),
        ],
    );
    assert!(String::from_utf8(output.0.lock().unwrap().clone())
        .unwrap()
        .contains("outside declared scope"));
    assert_eq!(result.summary.unwrap().files_outside_scope, 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("untracked")).unwrap(),
        "untracked \n"
    );
}
#[test]
fn dependency_requires_justification_and_logs_it() {
    let dir = repo();
    fs::write(dir.path().join("Cargo.toml"), "[dependencies]\n").unwrap();
    let output = Output::default();
    let tools = tools(dir.path(), "needed for parsing\n", output.clone());
    let result = run(
        dir.path(),
        Profile::Build,
        &tools,
        vec![
            response(vec![
                command(),
                scope(&["Cargo.toml", "tests/a.rs"]),
                write("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
                write("tests/a.rs", "new test"),
                command(),
            ]),
            response(vec![]),
        ],
    );
    assert!(String::from_utf8(output.0.lock().unwrap().clone())
        .unwrap()
        .contains("justification"));
    assert!(session_state(&tools)
        .unwrap()
        .log
        .entries
        .iter()
        .any(|e| e.kind == ToolKind::DependencyJustification
            && e.detail.contains("needed for parsing")));
    assert_eq!(result.summary.unwrap().dependencies_added, 1);
}
#[test]
fn ask_profile_preserves_repo() {
    let dir = repo();
    let root = dir.path();
    let files = ["src/a.rs", "tests/a.rs", "dirty", "untracked"];
    let before: Vec<_> = files
        .iter()
        .map(|p| fs::read(root.join(p)).unwrap())
        .collect();
    let index = fs::read(root.join(".git/index")).unwrap();
    let status = git(root, &["status", "--porcelain"]);
    let tools = tools(root, "", Output::default());
    let result = run(
        root,
        Profile::Ask,
        &tools,
        vec![
            response(vec![scope(&["src/a.rs"]), write("src/a.rs", "bad")]),
            response(vec![]),
        ],
    );
    assert!(result
        .messages
        .iter()
        .any(|m| m.content.contains("disabled")));
    for (p, b) in files.iter().zip(before) {
        assert_eq!(fs::read(root.join(p)).unwrap(), b);
    }
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git(root, &["status", "--porcelain"]), status);
}
#[test]
fn summary_matches_scripted_usage() {
    let dir = repo();
    let tools = tools(dir.path(), "", Output::default());
    let result = run(
        dir.path(),
        Profile::Build,
        &tools,
        vec![response(vec![scope(&["src/a.rs"])]), response(vec![])],
    );
    let s = result.summary.unwrap();
    assert_eq!((s.steps, s.tokens_in, s.tokens_out), (2, 14, 6));
}
#[test]
fn edit_before_scope_is_rejected() {
    let dir = repo();
    let tools = tools(dir.path(), "yes\n", Output::default());
    let result = run(
        dir.path(),
        Profile::Build,
        &tools,
        vec![
            response(vec![command(), write("src/a.rs", "bad")]),
            response(vec![]),
        ],
    );
    assert!(result
        .messages
        .iter()
        .any(|m| m.content.contains("declare_scope must be called")));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.rs")).unwrap(),
        "partially staged\n"
    );
}
