use devcode::agent::model::{Model, ModelError, Request, ToolSchema};
use devcode::agent::openai_compat::OpenAiCompat;
use devcode::config::{parse_config, ProviderConfig, ProviderKind};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;

fn config(url: String) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: url,
        api_key_env: String::new(),
        connector: None,
        model: "model".into(),
        context_window: 1000,
        max_rpm: None,
        extra_body: Default::default(),
    }
}
fn request() -> Request {
    Request {
        system_prompt: "system".into(),
        messages: vec![],
        tools: vec![ToolSchema {
            name: "read".into(),
            description: "read file".into(),
            parameters: serde_json::json!({"type":"object"}),
        }],
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
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            // The client may already have closed the socket; Linux reports ENOTCONN, macOS does not.
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Write);
        }
    });
    (format!("http://{address}"), requests)
}
fn server(response: &'static str) -> String {
    scripted_server(vec![(200, response.to_owned())]).0
}
#[test]
fn parses_tool_call() {
    let url = server(
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"1","function":{"name":"read","arguments":"{}"}}]}}],"usage":{"prompt_tokens":2,"completion_tokens":3}}"#,
    );
    let result = OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .unwrap();
    assert_eq!(result.tool_calls[0].name, "read");
    assert_eq!(result.usage.input_tokens, 2);
    assert_eq!(result.usage.output_tokens, 3);
}
#[test]
fn rejects_invalid_arguments() {
    let url = server(
        r#"{"choices":[{"message":{"tool_calls":[{"id":"1","function":{"name":"read","arguments":"bad"}}]}}]}"#,
    );
    assert!(matches!(
        OpenAiCompat::new(config(url)).unwrap().complete(&request()),
        Err(ModelError::FormatError(_))
    ));
}
#[test]
fn rejects_unsupported_provider() {
    let err = parse_config(r#"{"provider":{"kind":"bad","base_url":"x","api_key_env":"","model":"x","context_window":1}}"#).unwrap_err();
    assert!(err.to_string().contains("openai_compat"));
}

#[test]
fn ignores_reasoning_content() {
    let url = server(r#"{"choices":[{"message":{"content":"42","reasoning_content":"private"}}]}"#);
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .unwrap();
    assert_eq!(response.text.as_deref(), Some("42"));
    let url = server(r#"{"choices":[{"message":{"content":null,"reasoning_content":"private"}}]}"#);
    assert!(matches!(
        OpenAiCompat::new(config(url)).unwrap().complete(&request()),
        Err(ModelError::FormatError(_))
    ));
}

const OK: &str = r#"{"choices":[{"message":{"content":"ok"}}]}"#;

#[test]
fn request_body_and_no_authorization() {
    let (url, requests) = scripted_server(vec![(200, OK.into())]);
    OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .unwrap();
    let requests = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&requests[0]);
    let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["model"], "model");
    assert_eq!(
        body["messages"],
        serde_json::json!([{"role":"system","content":"system"}])
    );
    assert_eq!(
        body["tools"],
        serde_json::json!([{"type":"function","function":{"name":"read","description":"read file","parameters":{"type":"object"}}}])
    );
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["stream"], false);
}
#[test]
fn authorization_and_error_redaction() {
    const KEY: &str = "fake-test-key-123";
    std::env::set_var("DEVCODE_TEST_FAKE_KEY", KEY);
    let (url, requests) = scripted_server(vec![(401, format!("rejected {KEY}"))]);
    let mut cfg = config(url);
    cfg.api_key_env = "DEVCODE_TEST_FAKE_KEY".into();
    let err = OpenAiCompat::new(cfg)
        .unwrap()
        .complete(&request())
        .unwrap_err()
        .to_string();
    assert!(!err.contains(KEY));
    let requests = requests.lock().unwrap();
    assert!(String::from_utf8_lossy(&requests[0])
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {KEY}")));
    assert_eq!(requests.len(), 1);
    std::env::remove_var("DEVCODE_TEST_FAKE_KEY");
}
#[test]
fn retry_429_then_success() {
    let (url, requests) = scripted_server(vec![(429, "busy".into()), (200, OK.into())]);
    assert!(OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .is_ok());
    assert_eq!(requests.lock().unwrap().len(), 2);
}
#[test]
fn retry_429_cap() {
    let (url, requests) = scripted_server(vec![(429, "busy".into()); 6]);
    assert!(OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .is_err());
    assert_eq!(requests.lock().unwrap().len(), 6);
}
#[test]
fn retry_5xx_cap() {
    let (url, requests) = scripted_server(vec![(500, "error".into()); 3]);
    assert!(OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .is_err());
    assert_eq!(requests.lock().unwrap().len(), 3);
}
#[test]
fn connect_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert!(matches!(
        OpenAiCompat::new(config(format!("http://{address}")))
            .unwrap()
            .complete(&request()),
        Err(ModelError::Network(_))
    ));
}
#[test]
fn empty_response_is_format_error() {
    let url = server(r#"{"choices":[{"message":{"content":null}}]}"#);
    assert!(matches!(
        OpenAiCompat::new(config(url)).unwrap().complete(&request()),
        Err(ModelError::FormatError(_))
    ));
}
#[test]
fn nim_preset_fills_missing_fields() {
    use clap::Parser;
    let cli = devcode::Cli::parse_from(["devcode", "--provider", "nim"]);
    let mut config =
        parse_config(r#"{"provider":{"kind":"openai_compat","model":"m","context_window":1}}"#)
            .unwrap();
    cli.apply_provider_preset(&mut config);
    let provider = config.provider.unwrap();
    assert_eq!(provider.base_url, "https://integrate.api.nvidia.com/v1");
    assert_eq!(provider.api_key_env, "NVIDIA_API_KEY");
}
#[test]
fn rejects_key_value_in_config() {
    assert!(parse_config(r#"{"provider":{"kind":"openai_compat","base_url":"x","api_key_env":"nvapi-secret-value","model":"m","context_window":1}}"#).is_err());
}

#[test]
fn extra_body_fake_server() {
    let (url, requests) = scripted_server(vec![(
        200,
        r#"{"choices":[{"message":{"content":"ok"}}]}"#.into(),
    )]);
    let mut cfg = config(url);
    cfg.extra_body.insert(
        "chat_template_kwargs".into(),
        serde_json::json!({"enable_thinking":false}),
    );
    OpenAiCompat::new(cfg)
        .unwrap()
        .complete(&request())
        .unwrap();
    let requests = requests.lock().unwrap();
    let data = &requests[0];
    let offset = data.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let body: serde_json::Value = serde_json::from_slice(&data[offset..]).unwrap();
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(body["model"], "model");
    assert!(body["messages"].is_array());
    assert!(body["tools"].is_array());
    assert_eq!(body["stream"], false);
}

#[test]
fn thought_signature_is_read_and_echoed_back() {
    let reply = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","extra_content":{"google":{"thought_signature":"sig-abc"}},"function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into()), (200, reply.into())]);
    let model = OpenAiCompat::new(config(url)).unwrap();
    let first = model.complete(&request()).unwrap();
    assert_eq!(
        first.tool_calls[0].thought_signature.as_deref(),
        Some("sig-abc")
    );
    let mut next = request();
    next.messages.push(devcode::agent::Message::assistant(
        "",
        first.tool_calls.clone(),
    ));
    model.complete(&next).unwrap();
    let sent = requests.lock().unwrap();
    let body = String::from_utf8_lossy(sent.last().unwrap()).into_owned();
    let json: serde_json::Value =
        serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        json.pointer("/messages/1/tool_calls/0/extra_content/google/thought_signature"),
        Some(&serde_json::json!("sig-abc"))
    );
}

#[test]
fn list_models_strips_gemini_prefix_sorts_and_dedupes() {
    let reply = r#"{"data":[{"id":"models/gemini-b"},{"id":"gemini-a"},{"id":"gemini-b"}]}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into())]);
    let ids = OpenAiCompat::new(config(url))
        .unwrap()
        .list_models()
        .unwrap();
    assert_eq!(ids, vec!["gemini-a", "gemini-b"]);
    let sent = String::from_utf8_lossy(&requests.lock().unwrap()[0]).into_owned();
    assert!(sent.starts_with("GET /models "), "{sent}");
}

#[test]
fn list_models_reports_http_errors() {
    let (url, _) = scripted_server(vec![(401, r#"{"error":"bad key"}"#.into())]);
    let err = OpenAiCompat::new(config(url))
        .unwrap()
        .list_models()
        .unwrap_err();
    assert!(err.to_string().contains("HTTP 401"), "{err}");
}
