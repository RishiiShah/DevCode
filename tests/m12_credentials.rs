use devcode::credentials::{account_lines, load, save};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

fn force_file_store() {
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
}

#[test]
fn account_lines_mark_active_and_sources() {
    let mut entries = BTreeMap::new();
    entries.insert("nim".into(), "x".into());
    entries.insert("custom-lab".into(), "y".into());
    let lines = account_lines(&entries, Some("nim"));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("nim  stored") && l.contains('*')),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.starts_with("gemini  ")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("openai  ")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("openrouter  ")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("custom-lab  stored (custom)")),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains('x') || l.contains('y')));
}

#[test]
fn storage_modes_atomic_and_insecure_refusal() {
    force_file_store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config");
    let mut values = BTreeMap::new();
    values.insert("nim".into(), "test-value".into());
    save(&path, &values).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path.join("credentials.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(load(&path).unwrap(), values);
    std::fs::set_permissions(
        path.join("credentials.json"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert!(load(&path).unwrap_err().to_string().contains("chmod 600"));
}

#[test]
fn lexical_guard_denies_credentials() {
    use devcode::approval::{check_shell_command, CommandCheck};
    let root = tempfile::tempdir().unwrap();
    for cmd in [
        "cat $DEVCODE_CONFIG_DIR/credentials.json",
        "cat ~/.config/devcode/credentials.json",
    ] {
        assert!(matches!(
            check_shell_command(cmd, root.path(), &[]),
            CommandCheck::Denied(_)
        ));
    }
}

use std::io::Write;
use std::process::{Command, Stdio};
fn auth(
    root: &std::path::Path,
    dir: &std::path::Path,
    args: &[&str],
    input: Option<&str>,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root)
        .env("DEVCODE_CONFIG_DIR", dir)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}
#[test]
fn login_list_logout_and_reject_argument() {
    force_file_store();
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    let secret = "fake-key-UNIQUE-987";
    let login = auth(
        root.path(),
        &dir,
        &["auth", "login", "nim", "--with-stdin"],
        Some(&format!("{}\n", secret)),
    );
    assert!(
        login.status.success(),
        "{}",
        String::from_utf8_lossy(&login.stderr)
    );
    assert!(!String::from_utf8_lossy(&login.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&login.stderr).contains(secret));
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(dir.join("credentials.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let listed = auth(root.path(), &dir, &["auth", "list"], None);
    assert!(listed.status.success());
    let output = String::from_utf8(listed.stdout).unwrap();
    assert!(
        output.contains("nim  stored"),
        "expected account status row, got: {output}"
    );
    assert!(output.contains("openai  missing") || output.contains("openai  env"));
    assert!(!output.contains(secret));
    let bad = auth(root.path(), &dir, &["auth", "login", "nim", secret], None);
    assert!(!bad.status.success());
    assert!(!String::from_utf8_lossy(&bad.stderr).contains(secret));
    for _ in 0..2 {
        assert!(auth(root.path(), &dir, &["auth", "logout", "nim"], None)
            .status
            .success());
    }
    assert!(load(&dir).unwrap().is_empty());
}
#[test]
fn auth_default_connector_and_no_tty_refusal() {
    force_file_store();
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    std::fs::write(root.path().join(".devcode.json"),r#"{"provider":{"kind":"openai_compat","connector":"named","model":"x","context_window":10}}"#).unwrap();
    let no_tty = auth(root.path(), &dir, &["auth", "login"], None);
    assert!(!no_tty.status.success());
    let login = auth(
        root.path(),
        &dir,
        &["auth", "login", "--with-stdin"],
        Some("fake-key\n"),
    );
    assert!(login.status.success());
    assert!(load(&dir).unwrap().contains_key("named"));
}
#[test]
fn reject_credentials_inside_workspace() {
    let root = tempfile::tempdir().unwrap();
    let output = auth(
        root.path(),
        &root.path().join("credentials"),
        &["auth", "login", "nim", "--with-stdin"],
        Some("fake\n"),
    );
    assert!(!output.status.success());
    assert!(!root.path().join("credentials").exists());
}
#[test]
fn inaccessible_file_reports_fix_via_cli() {
    force_file_store();
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    let mut values = BTreeMap::new();
    values.insert("nim".into(), "fake-key".into());
    save(&dir, &values).unwrap();
    std::fs::set_permissions(
        dir.join("credentials.json"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let output = auth(root.path(), &dir, &["auth", "list"], None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("chmod 600"));
}

#[test]
fn run_command_denies_credentials_path() {
    use devcode::agent::{ToolCall, ToolRegistry};
    use devcode::approval::ApprovalPrompt;
    use devcode::tools::{registry_with_context, MutationContext};
    use std::io::Cursor;
    let root = tempfile::tempdir().unwrap();
    let tools: ToolRegistry = registry_with_context(MutationContext::new(
        root.path().to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new(), false, true),
    ));
    let err = tools
        .execute(&ToolCall {
            id: "1".into(),
            thought_signature: None,
            name: "run_command".into(),
            arguments: serde_json::json!({"command":"cat $DEVCODE_CONFIG_DIR/credentials.json"})
                .to_string(),
        })
        .unwrap_err();
    assert!(err.contains("credentials"));
}

#[test]
fn scripted_run_redacts_tool_output_and_arguments() {
    use devcode::agent::{
        run_agent_with_config, LoopLimits, Model, ModelError, Request, Response, ScriptedModel,
        Tool, ToolCall, ToolRegistry, Usage,
    };
    struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            "echo"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn execute(&self, args: &str) -> Result<String, String> {
            Ok(args.to_string())
        }
    }
    struct Protected(ScriptedModel);
    impl Model for Protected {
        fn complete(&self, req: &Request) -> Result<Response, ModelError> {
            self.0.complete(req)
        }
        fn redact_sensitive(&self, text: &str) -> String {
            text.replace("synthetic-secret-789", "[REDACTED]")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut tools = ToolRegistry::new();
    tools.register(Echo);
    let script = Protected(ScriptedModel::new(vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "1".into(),
                thought_signature: None,
                name: "read_file".into(),
                arguments: "synthetic-secret-789".into(),
            }],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("synthetic-secret-789".into()),
            tool_calls: vec![],
            usage: Usage::default(),
        }),
    ]));
    let result = run_agent_with_config(
        devcode::Profile::Ask,
        &script,
        &tools,
        "safe",
        &LoopLimits {
            trace: true,
            ..Default::default()
        },
        root.path(),
        &devcode::Config::default(),
    )
    .unwrap();
    assert!(!format!("{:?}", result).contains("synthetic-secret-789"));
    let trace = std::fs::read_to_string(root.path().join(".devcode/trace.jsonl")).unwrap();
    assert!(!trace.contains("synthetic-secret-789"));
}
