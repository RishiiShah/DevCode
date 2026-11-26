use devcode::auth::{
    login_gemini, save_oauth, valid_gemini_oauth, LoopbackServer, Pkce, StoredOAuth, OAUTH_FILE,
};
use devcode::credentials::{load, save};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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

fn pkce() -> Pkce {
    Pkce {
        verifier: "verifier-test".into(),
        challenge: "challenge-test".into(),
        state: "state-test".into(),
    }
}

#[test]
fn exchange_saves_oauth_file_keeps_api_key() {
    let (url, requests) = scripted_server(vec![(
        200,
        r#"{"access_token":"ya29.test","refresh_token":"refresh-keep","expires_in":3600}"#.into(),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let mut existing = BTreeMap::new();
    existing.insert("gemini".into(), "api-key-keep".into());
    existing.insert("nim".into(), "keep-me".into());
    save(dir.path(), &existing).unwrap();
    login_gemini(dir.path(), "auth-code-xyz", &url, &pkce()).unwrap();
    let creds = load(dir.path()).unwrap();
    assert_eq!(
        creds.get("gemini").map(String::as_str),
        Some("api-key-keep")
    );
    assert_eq!(creds.get("nim").map(String::as_str), Some("keep-me"));
    let oauth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(OAUTH_FILE)).unwrap())
            .unwrap();
    assert_eq!(oauth["access_token"], "ya29.test");
    assert_eq!(oauth["refresh_token"], "refresh-keep");
    assert!(oauth.get("expires_at").and_then(|v| v.as_u64()).is_some());
    let captured = requests.lock().unwrap();
    let body = String::from_utf8_lossy(&captured[0]);
    let form = body.split("\r\n\r\n").nth(1).unwrap();
    assert!(form.contains("auth-code-xyz"), "{form}");
    assert!(form.contains("verifier-test"), "{form}");
    assert!(!form.contains("ya29.test"), "{form}");
}

#[test]
fn exchange_failure_preserves_existing() {
    let dir = tempfile::tempdir().unwrap();
    let mut existing = BTreeMap::new();
    existing.insert("gemini".into(), "old-key".into());
    save(dir.path(), &existing).unwrap();
    let (url, _) = scripted_server(vec![(400, r#"{"error":"invalid_grant"}"#.into())]);
    let err = login_gemini(dir.path(), "bad-code", &url, &pkce()).unwrap_err();
    assert!(
        err.contains("400") || err.contains("invalid_grant"),
        "{err}"
    );
    let creds = load(dir.path()).unwrap();
    assert_eq!(creds.get("gemini").map(String::as_str), Some("old-key"));
    assert!(!dir.path().join(OAUTH_FILE).exists());
}

#[test]
fn authorize_url_contains_client_challenge_method_and_redirect() {
    let pkce = devcode::auth::generate_pkce().unwrap();
    let url = devcode::auth::authorize_url(&pkce);
    assert!(url.contains(devcode::auth::CLIENT_ID), "{url}");
    assert!(url.contains("code_challenge_method=S256"), "{url}");
    assert!(
        url.contains("redirect_uri=https%3A%2F%2Fcodeassist.google.com%2Fauthcode"),
        "{url}"
    );
    assert!(url.contains("cloud-platform"), "{url}");
    assert!(url.contains("userinfo.email"), "{url}");
    assert!(url.contains("userinfo.profile"), "{url}");
}

#[test]
fn oauth_rejects_non_gemini() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &dir)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "login", "nim", "--oauth", "--with-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"should-not-be-read\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("oauth login is only supported for gemini"),
        "{err}"
    );
    assert!(!dir.join("credentials.json").exists());
    assert!(!dir.join(OAUTH_FILE).exists());
}

#[test]
fn expired_oauth_refreshes_automatically_and_preserves_refresh_token() {
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    let dir = tempfile::tempdir().unwrap();
    save_oauth(
        dir.path(),
        &StoredOAuth {
            access_token: "expired-access".into(),
            refresh_token: Some("keep-refresh".into()),
            expires_at: Some(1),
            project_id: Some("proj-123".into()),
        },
    )
    .unwrap();
    let (url, requests) = scripted_server(vec![(
        200,
        r#"{"access_token":"fresh-access","expires_in":3600}"#.into(),
    )]);
    let refreshed = valid_gemini_oauth(dir.path(), &url).unwrap().unwrap();
    assert_eq!(refreshed.access_token, "fresh-access");
    assert_eq!(refreshed.refresh_token.as_deref(), Some("keep-refresh"));
    assert_eq!(refreshed.project_id.as_deref(), Some("proj-123"));
    let captured = requests.lock().unwrap();
    let form = String::from_utf8_lossy(&captured[0]);
    assert!(form.contains("grant_type=refresh_token"), "{form}");
    assert!(form.contains("refresh_token=keep-refresh"), "{form}");
}

#[test]
fn loopback_server_receives_code_and_rejects_bad_state() {
    let server = LoopbackServer::bind().unwrap();
    let uri = server.redirect_uri.clone();
    let addr = uri
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let handle =
        thread::spawn(move || server.wait_for_code("expected-state", Duration::from_secs(5)));
    let mut stream = TcpStream::connect(&addr).unwrap();
    write!(
        stream,
        "GET /oauth2callback?code=cb-auth-code&state=expected-state HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    assert!(response.contains("200 OK"), "{response}");
    assert_eq!(handle.join().unwrap().unwrap(), "cb-auth-code");

    let server2 = LoopbackServer::bind().unwrap();
    let addr2 = server2
        .redirect_uri
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let handle2 =
        thread::spawn(move || server2.wait_for_code("expected-state", Duration::from_secs(5)));
    let mut stream2 = TcpStream::connect(&addr2).unwrap();
    write!(
        stream2,
        "GET /oauth2callback?code=cb-auth-code&state=wrong-state HTTP/1.1\r\nHost: {addr2}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream2.flush().unwrap();
    assert!(handle2
        .join()
        .unwrap()
        .unwrap_err()
        .contains("state mismatch"));
}
