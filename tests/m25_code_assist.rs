use devcode::agent::code_assist::CodeAssist;
use devcode::agent::model::{
    CancelToken, Message, Model, ModelError, Request, StreamChunk, ToolSchema,
};
use devcode::agent::ToolCall;
use devcode::auth::{load_oauth, save_oauth, StoredOAuth, OAUTH_FILE, OAUTH_SLOT};
use devcode::config::{ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn read_http_request(stream: &mut TcpStream) -> (String, Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
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
    let headers = String::from_utf8_lossy(&data).into_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    data.resize(header_end + length, 0);
    if length > 0 {
        stream.read_exact(&mut data[header_end..]).unwrap();
    }
    let body = if length > 0 {
        serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    (headers, body)
}

fn write_json(stream: &mut TcpStream, status: u16, body: &Value) {
    let text = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
    .unwrap();
    stream.flush().unwrap();
    let _ = stream.shutdown(Shutdown::Write);
}

fn write_sse(stream: &mut TcpStream, events: &[Value]) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    for ev in events {
        let chunk = format!("data: {ev}\n\n");
        write!(stream, "{:x}\r\n{chunk}\r\n", chunk.len()).unwrap();
        stream.flush().unwrap();
    }
    stream.write_all(b"0\r\n\r\n").unwrap();
    stream.flush().unwrap();
    let _ = stream.shutdown(Shutdown::Write);
}

fn ca_config(url: String) -> ProviderConfig {
    let mut extra = serde_json::Map::new();
    extra.insert("reasoning_effort".into(), json!("high"));
    ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: url,
        api_key_env: String::new(),
        connector: Some("gemini".into()),
        model: "gemini-3-flash-preview".into(),
        context_window: 100_000,
        max_rpm: None,
        extra_body: extra,
    }
}

fn sample_request() -> Request {
    Request {
        system_prompt: "be concise".into(),
        messages: vec![Message::user("hi")],
        tools: vec![ToolSchema {
            name: "read_file".into(),
            description: "read".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
    }
}

#[test]
fn code_assist_onboards_streams_reasoning_and_tool_calls() {
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    let dir = tempfile::tempdir().unwrap();
    save_oauth(
        dir.path(),
        &StoredOAuth {
            access_token: "ya29.oauth-secret".into(),
            refresh_token: Some("rt-1".into()),
            expires_at: None,
            project_id: None,
        },
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        // 1. loadCodeAssist
        let (mut s1, _) = listener.accept().unwrap();
        let (h1, b1) = read_http_request(&mut s1);
        recorded.lock().unwrap().push((h1, b1));
        write_json(
            &mut s1,
            200,
            &json!({"cloudaicompanionProject": "ca-proj-42"}),
        );

        // 2. streamGenerateContent?alt=sse
        let (mut s2, _) = listener.accept().unwrap();
        let (h2, b2) = read_http_request(&mut s2);
        recorded.lock().unwrap().push((h2, b2));
        write_sse(
            &mut s2,
            &[
                json!({
                    "response": {
                        "candidates": [{
                            "content": {
                                "parts": [{"text": "thinking ya29.oauth-secret", "thought": true}]
                            }
                        }]
                    }
                }),
                json!({
                    "response": {
                        "candidates": [{
                            "content": {
                                "parts": [
                                    {"text": "let me check"},
                                    {"functionCall": {"name": "read_file", "args": {"path": "a.rs"}}, "thoughtSignature": "sig-9"}
                                ]
                            }
                        }],
                        "usageMetadata": {"promptTokenCount": 21, "candidatesTokenCount": 9}
                    }
                }),
            ],
        );
    });

    let ca = CodeAssist::new_at(
        ca_config(format!("http://{address}")),
        dir.path().to_path_buf(),
        format!("http://{address}/token"),
    )
    .unwrap();
    let mut text = String::new();
    let mut reasoning = String::new();
    let resp = ca
        .complete_stream_cancellable(&sample_request(), None, &mut |chunk| match chunk {
            StreamChunk::Text(s) => text.push_str(s),
            StreamChunk::Reasoning(s) => {
                assert!(!s.contains("ya29.oauth-secret"));
                reasoning.push_str(s);
            }
        })
        .unwrap();

    assert_eq!(text, "let me check");
    assert_eq!(reasoning, "thinking [REDACTED]");
    assert_eq!(resp.text.as_deref(), Some("let me check"));
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(
        resp.tool_calls[0].thought_signature.as_deref(),
        Some("sig-9")
    );
    assert_eq!(resp.usage.input_tokens, 21);
    assert_eq!(resp.usage.output_tokens, 9);

    // Project ID was cached to oauth-gemini.json.
    assert_eq!(
        load_oauth(dir.path())
            .unwrap()
            .unwrap()
            .project_id
            .as_deref(),
        Some("ca-proj-42")
    );

    let captured = requests.lock().unwrap();
    assert!(captured[0].0.contains("POST /v1internal:loadCodeAssist"));
    assert!(captured[1]
        .0
        .contains("POST /v1internal:streamGenerateContent?alt=sse"));
    assert!(captured[1]
        .0
        .to_ascii_lowercase()
        .contains("authorization: bearer ya29.oauth-secret"));
    assert_eq!(captured[1].1["project"], "ca-proj-42");
    assert_eq!(
        captured[1].1["request"]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "HIGH"
    );
}

#[test]
fn code_assist_refreshes_on_401_and_supports_cancel() {
    std::env::set_var("DEVCODE_CREDENTIALS_STORE", "file");
    let dir = tempfile::tempdir().unwrap();
    save_oauth(
        dir.path(),
        &StoredOAuth {
            access_token: "stale-tok".into(),
            refresh_token: Some("rt-2".into()),
            expires_at: None,
            project_id: Some("proj-cached".into()),
        },
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        // 1. 401 from generateContent
        let (mut s1, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut s1);
        write_json(&mut s1, 401, &json!({"error": "invalid_token"}));
        // 2. Refresh token POST
        let (mut s2, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut s2);
        write_json(
            &mut s2,
            200,
            &json!({"access_token": "fresh-tok", "expires_in": 3600}),
        );
        // 3. Retried generateContent succeeds
        let (mut s3, _) = listener.accept().unwrap();
        let (h3, _) = read_http_request(&mut s3);
        assert!(h3.to_ascii_lowercase().contains("bearer fresh-tok"));
        write_json(
            &mut s3,
            200,
            &json!({
                "response": {
                    "candidates": [{"content": {"parts": [{"text": "recovered"}]}}],
                    "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 1}
                }
            }),
        );
    });

    let ca = CodeAssist::new_at(
        ca_config(format!("http://{address}")),
        dir.path().to_path_buf(),
        format!("http://{address}/token"),
    )
    .unwrap();
    let resp = ca.complete(&sample_request()).unwrap();
    assert_eq!(resp.text.as_deref(), Some("recovered"));
    assert_eq!(
        load_oauth(dir.path()).unwrap().unwrap().access_token,
        "fresh-tok"
    );

    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        ca.complete_cancellable(&sample_request(), Some(&cancel)),
        Err(ModelError::Cancelled)
    ));
}

#[test]
fn auth_switch_and_list_support_gemini_oauth_slot() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    save_oauth(
        &dir,
        &StoredOAuth {
            access_token: "ya29.secret-abc".into(),
            refresh_token: Some("rt-x".into()),
            expires_at: None,
            project_id: None,
        },
    )
    .unwrap();

    let switched = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &dir)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "switch", OAUTH_SLOT])
        .output()
        .unwrap();
    assert!(switched.status.success());
    assert_eq!(
        String::from_utf8_lossy(&switched.stdout).trim(),
        "active gemini:oauth"
    );

    let listed = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &dir)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "list"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    let out = String::from_utf8_lossy(&listed.stdout);
    assert!(out.contains("gemini:oauth  stored (oauth) *"), "{out}");
    assert!(!out.contains("ya29.secret-abc"));
}

fn function_response_names(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("functionResponse")
                .and_then(|fr| fr.get("name"))
                .and_then(Value::as_str)
            {
                out.push(name.to_owned());
            }
            map.values().for_each(|v| function_response_names(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| function_response_names(v, out)),
        _ => {}
    }
}

#[test]
fn repeated_fallback_tool_ids_resolve_to_the_latest_call() {
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    save_oauth(
        &dir,
        &StoredOAuth {
            access_token: "tok".into(),
            refresh_token: None,
            expires_at: None,
            project_id: Some("p".into()),
        },
    )
    .unwrap();
    let ca = CodeAssist::new_at(
        ca_config("http://127.0.0.1:9".into()),
        dir,
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    let call = |name: &str| ToolCall {
        id: "ca_0".into(),
        name: name.into(),
        arguments: "{}".into(),
        thought_signature: None,
    };
    let mut req = sample_request();
    req.messages
        .push(Message::assistant("", vec![call("read_file")]));
    req.messages.push(Message::tool("one", "ca_0"));
    req.messages
        .push(Message::assistant("", vec![call("write_file")]));
    req.messages.push(Message::tool("two", "ca_0"));
    let body = ca.request_body(&req, Some("p"));
    let mut names = Vec::new();
    function_response_names(&body, &mut names);
    assert_eq!(names, vec!["read_file", "write_file"]);
}

#[test]
fn logout_gemini_oauth_removes_stored_tokens() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let dir = storage.path().join("config");
    save_oauth(
        &dir,
        &StoredOAuth {
            access_token: "ya29.secret-abc".into(),
            refresh_token: Some("rt-x".into()),
            expires_at: None,
            project_id: None,
        },
    )
    .unwrap();
    assert!(dir.join(OAUTH_FILE).exists());
    let out = Command::new(env!("CARGO_BIN_EXE_devcode"))
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", &dir)
        .env("DEVCODE_CREDENTIALS_STORE", "file")
        .args(["auth", "logout", OAUTH_SLOT])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join(OAUTH_FILE).exists());
}
