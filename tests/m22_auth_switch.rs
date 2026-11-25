use devcode::agent::anthropic::Anthropic;
use devcode::agent::model::{Model, Request};
use devcode::agent::openai_compat::OpenAiCompat;
use devcode::config::{Config, ProviderConfig, ProviderKind};
use devcode::credentials::{self, load};
use devcode::{load_user_settings, Profile, Session, UserSettings};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const PLAIN: &str = "fake-plain-m22-aaa";
const WORK: &str = "fake-work-m22-bbb";
const ENV_KEY: &str = "fake-env-m22-ccc";
const CONNECT: &str = "fake-connect-m22-ddd";
const ENV_NAME: &str = "DEVCODE_M22_SLOT_KEY";

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

fn auth(root: &Path, dir: &Path, args: &[&str], input: Option<&str>) -> std::process::Output {
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

fn assert_no_secrets(output: &std::process::Output) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in [PLAIN, WORK, ENV_KEY, CONNECT] {
        assert!(!text.contains(secret), "credential leaked:\n{text}");
    }
}

fn file_bytes(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[test]
fn auth_switch_selects_a_stored_slot_without_touching_the_provider() {
    let _guard = env_lock();
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    let settings = dir.join("settings.json");
    let creds = dir.join("credentials.json");

    let login_plain = auth(
        root.path(),
        &dir,
        &["auth", "login", "nim", "--with-stdin"],
        Some(&format!("{PLAIN}\n")),
    );
    assert!(
        login_plain.status.success(),
        "{}",
        String::from_utf8_lossy(&login_plain.stderr)
    );
    assert_no_secrets(&login_plain);
    let login_work = auth(
        root.path(),
        &dir,
        &["auth", "login", "nim:work", "--with-stdin"],
        Some(&format!("{WORK}\n")),
    );
    assert!(
        login_work.status.success(),
        "{}",
        String::from_utf8_lossy(&login_work.stderr)
    );
    assert_no_secrets(&login_work);
    let stored = load(&dir).unwrap();
    assert_eq!(stored.get("nim").map(String::as_str), Some(PLAIN));
    assert_eq!(stored.get("nim:work").map(String::as_str), Some(WORK));
    assert!(!settings.exists(), "login must not create active_slot");

    let creds_before = file_bytes(&creds).unwrap();
    let missing = auth(root.path(), &dir, &["auth", "switch", "nim:missing"], None);
    assert!(!missing.status.success());
    assert_no_secrets(&missing);
    assert_eq!(file_bytes(&creds).unwrap(), creds_before);
    assert!(!settings.exists(), "missing switch must not write settings");

    let switched = auth(root.path(), &dir, &["auth", "switch", "nim:work"], None);
    assert!(
        switched.status.success(),
        "{}",
        String::from_utf8_lossy(&switched.stderr)
    );
    assert_eq!(
        String::from_utf8(switched.stdout.clone()).unwrap().trim(),
        "active nim:work"
    );
    assert_no_secrets(&switched);
    let after_switch = load_user_settings(&dir).unwrap();
    assert_eq!(after_switch.active_slot.as_deref(), Some("nim:work"));
    assert!(after_switch.provider.is_none());
    assert_eq!(file_bytes(&creds).unwrap(), creds_before);

    let listed = auth(root.path(), &dir, &["auth", "list"], None);
    assert!(listed.status.success());
    assert_no_secrets(&listed);
    let list = String::from_utf8(listed.stdout).unwrap();
    assert!(
        list.lines()
            .any(|line| line.starts_with("nim:work  stored") && line.contains('*')),
        "{list}"
    );
    assert!(
        list.lines()
            .any(|line| line.starts_with("nim ") && !line.contains('*')),
        "{list}"
    );

    let settings_before_missing = file_bytes(&settings).unwrap();
    let missing_again = auth(root.path(), &dir, &["auth", "switch", "nim:missing"], None);
    assert!(!missing_again.status.success());
    assert_no_secrets(&missing_again);
    assert_eq!(file_bytes(&creds).unwrap(), creds_before);
    assert_eq!(file_bytes(&settings).unwrap(), settings_before_missing);

    let relogin = auth(
        root.path(),
        &dir,
        &["auth", "login", "nim:work", "--with-stdin"],
        Some(&format!("{WORK}\n")),
    );
    assert!(
        relogin.status.success(),
        "{}",
        String::from_utf8_lossy(&relogin.stderr)
    );
    assert_no_secrets(&relogin);
    assert_eq!(
        load_user_settings(&dir).unwrap().active_slot.as_deref(),
        Some("nim:work")
    );
    assert!(load_user_settings(&dir).unwrap().provider.is_none());

    let logout_plain = auth(root.path(), &dir, &["auth", "logout", "nim"], None);
    assert!(logout_plain.status.success());
    assert_no_secrets(&logout_plain);
    assert!(!load(&dir).unwrap().contains_key("nim"));
    assert_eq!(
        load(&dir).unwrap().get("nim:work").map(String::as_str),
        Some(WORK)
    );
    assert_eq!(
        load_user_settings(&dir).unwrap().active_slot.as_deref(),
        Some("nim:work")
    );

    let logout_active = auth(root.path(), &dir, &["auth", "logout", "nim:work"], None);
    assert!(logout_active.status.success());
    assert_no_secrets(&logout_active);
    assert!(!load(&dir).unwrap().contains_key("nim:work"));
    assert!(load_user_settings(&dir).unwrap().active_slot.is_none());
    assert!(load_user_settings(&dir).unwrap().provider.is_none());
}

fn provider(url: &str) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: url.into(),
        api_key_env: ENV_NAME.into(),
        connector: Some("nim".into()),
        model: "model".into(),
        context_window: 1000,
        max_rpm: None,
        extra_body: Default::default(),
    }
}

fn scripted_server(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            let header_end = loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                data.push(byte[0]);
                if data.ends_with(b"\r\n\r\n") {
                    break data.len();
                }
            };
            let headers = String::from_utf8_lossy(&data);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap_or(0);
            data.resize(header_end + length, 0);
            stream.read_exact(&mut data[header_end..]).unwrap();
            recorded.lock().unwrap().push(data);
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Write);
        }
    });
    (format!("http://{address}"), requests)
}

fn bearer(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).to_ascii_lowercase()
}

#[test]
fn openai_compat_bearer_uses_the_active_slot_until_env_is_set() {
    let _guard = env_lock();
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    std::env::remove_var(ENV_NAME);
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    std::env::set_var("DEVCODE_CONFIG_DIR", &dir);

    let mut entries = BTreeMap::new();
    entries.insert("nim".into(), PLAIN.into());
    credentials::save(&dir, &entries).unwrap();
    devcode::save_user_settings(
        &dir,
        &UserSettings {
            active_slot: Some("nim:work".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let missing = match OpenAiCompat::new(provider("http://127.0.0.1:9")) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("missing slot key should fail"),
    };
    assert!(missing.contains("nim:work"), "{missing}");
    assert!(!missing.contains(PLAIN));
    assert!(!missing.contains(WORK));
    let mut anthropic = provider("http://127.0.0.1:9");
    anthropic.kind = ProviderKind::Anthropic;
    let anthropic_missing = match Anthropic::new(anthropic) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("missing slot key should fail"),
    };
    assert!(
        anthropic_missing.contains("nim:work"),
        "{anthropic_missing}"
    );
    assert!(!anthropic_missing.contains(PLAIN));

    entries.insert("nim:work".into(), WORK.into());
    credentials::save(&dir, &entries).unwrap();
    let (url, requests) = scripted_server(vec![
        (200, r#"{"choices":[{"message":{"content":"ok"}}]}"#.into()),
        (200, r#"{"choices":[{"message":{"content":"ok"}}]}"#.into()),
    ]);
    let request = Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
    };
    OpenAiCompat::new(provider(&url))
        .unwrap()
        .complete(&request)
        .unwrap();
    std::env::set_var(ENV_NAME, ENV_KEY);
    OpenAiCompat::new(provider(&url))
        .unwrap()
        .complete(&request)
        .unwrap();
    std::env::remove_var(ENV_NAME);

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    let work_headers = bearer(&recorded[0]);
    let env_headers = bearer(&recorded[1]);
    assert!(work_headers.contains(&format!("authorization: bearer {WORK}")));
    assert!(!work_headers.contains(PLAIN));
    assert!(!work_headers.contains(ENV_KEY));
    assert!(env_headers.contains(&format!("authorization: bearer {ENV_KEY}")));
    assert!(!env_headers.contains(WORK));
    assert!(!env_headers.contains(PLAIN));
}

#[test]
fn connect_named_slot_stores_the_key_and_selects_the_base_connector() {
    let _guard = env_lock();
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    std::env::set_var("DEVCODE_CONFIG_DIR", &dir);
    let mut session = Session::new(
        root.path().to_path_buf(),
        Config {
            provider: Some(ProviderConfig {
                kind: ProviderKind::OpenaiCompat,
                base_url: "http://127.0.0.1:9".into(),
                api_key_env: String::new(),
                connector: Some("gemini".into()),
                model: "fake".into(),
                context_window: 1000,
                max_rpm: None,
                extra_body: Default::default(),
            }),
            ..Default::default()
        },
        Profile::Ask,
        Box::new(devcode::agent::ScriptedModel::new(vec![])),
        devcode::agent::ToolRegistry::new(),
    );
    session
        .connect_with(Some("nim:work"), true, || Ok(CONNECT.into()))
        .unwrap();
    let stored = load(&dir).unwrap();
    assert_eq!(stored.get("nim:work").map(String::as_str), Some(CONNECT));
    assert!(!stored.contains_key("nim"));
    let settings = load_user_settings(&dir).unwrap();
    assert_eq!(settings.active_slot.as_deref(), Some("nim:work"));
    assert_eq!(
        settings
            .provider
            .as_ref()
            .and_then(|provider| provider.connector.as_deref()),
        Some("nim")
    );
}
