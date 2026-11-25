use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn force_file_store() {
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
}

fn pty() -> (std::fs::File, std::fs::File) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: pointers reference valid descriptors; null optional arguments are allowed by openpty.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    // SAFETY: openpty returns two owned, distinct file descriptors on success.
    unsafe {
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}
fn read_until(master: &mut std::fs::File, target: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        let mut fd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd points to one initialized pollfd for the duration of the call.
        if unsafe { libc::poll(&mut fd, 1, 100) } > 0 {
            let mut buf = [0; 1024];
            match master.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => bytes.extend_from_slice(&buf[..n]),
            };
            if String::from_utf8_lossy(&bytes).contains(target) {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
        }
    }
    panic!("expected prompt not received");
}
#[test]
fn typed_ahead_is_flushed_before_hidden_prompt() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let config = storage.path().join("config");
    let (mut master, slave) = pty();
    force_file_store();
    master.write_all(b"queued-fake-key\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &config)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "login", "nim"])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .spawn()
        .unwrap();
    let output = read_until(&mut master, "Paste the key now. Input is hidden.");
    assert!(!output.contains("fresh-fake-key"));
    master.write_all(b"fresh-fake-key\n").unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        devcode::credentials::load(&config)
            .unwrap()
            .get("nim")
            .unwrap(),
        "fresh-fake-key"
    );
}
#[test]
fn non_tty_connect_refused_and_missing_model_explained() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let config = storage.path().join("config");
    fs::write(root.path().join(".devcode.json"),r#"{"provider":{"kind":"openai_compat","connector":"nim","api_key_env":"M13_MISSING_TEST_KEY","model":"fake","context_window":100}}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &config)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .env_remove("M13_MISSING_TEST_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/connect nim\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("requires a TTY"));
    assert!(!config.exists());
    fs::write(
        root.path().join(".devcode.json"),
        r#"{"provider":{"kind":"openai_compat","connector":"nim"}}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &config)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/connect\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("provider.model is missing"));
}
#[test]
fn help_warns_not_to_paste_with_command() {
    let root = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", config.path())
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "login", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Do not paste the key with the command"));
}
#[test]
fn connections_lists_names_only() {
    force_file_store();
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let config = storage.path().join("config");
    let mut entries = std::collections::BTreeMap::new();
    entries.insert("nim".into(), "synthetic-secret-never-print".into());
    devcode::credentials::save(&config, &entries).unwrap();
    fs::write(
        root.path().join(".devcode.json"),
        r#"{"provider":{"kind":"openai_compat","model":"fake","context_window":100}}"#,
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &config)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/connections\n/exit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("nim  stored"),
        "expected account status row, got: {combined}"
    );
    assert!(
        combined.contains("gemini  missing") || combined.contains("gemini  env"),
        "presets without keys must still list"
    );
    assert!(!combined.contains("synthetic-secret-never-print"));
}

#[test]
fn tty_connect_rebuilds_provider_for_next_request() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut data = Vec::new();
        loop {
            let mut b = [0];
            stream.read_exact(&mut b).unwrap();
            data.push(b[0]);
            if data.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let headers = String::from_utf8(data).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        send.send(headers).unwrap();
        let response = r#"{"choices":[{"message":{"content":"safe response"}}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.len(),
            response
        )
        .unwrap();
    });
    force_file_store();
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let config = storage.path().join("config");
    fs::write(root.path().join(".devcode.json"),serde_json::json!({"provider":{"kind":"openai_compat","connector":"nim","api_key_env":"M13_MISSING_TEST_KEY","base_url":format!("http://{}",address),"model":"fake","context_window":10000}}).to_string()).unwrap();
    let (mut master, slave) = pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &config)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .env_remove("M13_MISSING_TEST_KEY")
        .arg("--trace")
        .arg("--plain")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave))
        .spawn()
        .unwrap();
    master.write_all(b"/connect nim\n").unwrap();
    let mut visible = read_until(&mut master, "Paste the key now. Input is hidden.");
    master.write_all(b"synthetic-secret-M13\n").unwrap();
    visible.push_str(&read_until(&mut master, "\r\n"));
    master.write_all(b"hello\n").unwrap();
    let headers = recv.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(headers.contains("Bearer synthetic-secret-M13"));
    visible.push_str(&read_until(&mut master, "safe response"));
    master.write_all(b"/connections\n").unwrap();
    visible.push_str(&read_until(&mut master, "nim  stored"));
    master.write_all(b"/exit\n").unwrap();
    assert!(child.wait().unwrap().success());
    server.join().unwrap();
    assert_eq!(
        devcode::credentials::load(&config)
            .unwrap()
            .get("nim")
            .unwrap(),
        "synthetic-secret-M13"
    );
    let settings =
        fs::read_to_string(config.join("settings.json")).expect("connect persists settings");
    assert!(
        settings.contains("\"connector\": \"nim\"") || settings.contains("\"connector\":\"nim\""),
        "settings.json should remember the connected provider, got: {settings}"
    );
    assert!(!visible.contains("synthetic-secret-M13"));
    let trace = fs::read_to_string(root.path().join(".devcode/trace.jsonl")).unwrap();
    assert!(!trace.contains("synthetic-secret-M13"));
}
