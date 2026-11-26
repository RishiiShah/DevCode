use devcode::agent::anthropic::Anthropic;
use devcode::agent::model::{Message, Model, ModelError, Request, ToolSchema};
use devcode::agent::ToolCall;
use devcode::config::{ProviderConfig, ProviderKind};
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
        model: "claude-test".into(),
        context_window: 100_000,
        max_rpm: None,
        extra_body: Default::default(),
    }
}

fn tool_request() -> Request {
    Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![ToolSchema {
            name: "read".into(),
            description: "read a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
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
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{}",
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

#[test]
fn request_headers_and_defaults() {
    let reply =
        r#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":2}}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into())]);
    let mut cfg = config(url);
    std::env::set_var("M20_ANTHROPIC_HDR_KEY", "test-key-anthropic");
    cfg.api_key_env = "M20_ANTHROPIC_HDR_KEY".into();
    let model = Anthropic::new(cfg).unwrap();
    model.complete(&tool_request()).unwrap();
    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&captured[0]);
    let headers = raw.split("\r\n\r\n").next().unwrap();
    assert!(headers
        .to_ascii_lowercase()
        .contains("x-api-key: test-key-anthropic"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("anthropic-version: 2023-06-01"));
    let body: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["max_tokens"], 4096);
    assert!(raw.contains("POST /v1/messages "));
    std::env::remove_var("M20_ANTHROPIC_HDR_KEY");
}

#[test]
fn parses_tool_use() {
    let reply = r#"{"content":[{"type":"tool_use","id":"tu_1","name":"read","input":{"path":"src/lib.rs"}}],"usage":{"input_tokens":3,"output_tokens":4}}"#;
    let (url, _) = scripted_server(vec![(200, reply.into())]);
    let response = Anthropic::new(config(url))
        .unwrap()
        .complete(&tool_request())
        .unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "tu_1");
    assert_eq!(response.tool_calls[0].name, "read");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.tool_calls[0].arguments).unwrap(),
        serde_json::json!({"path": "src/lib.rs"})
    );
    assert_eq!(response.usage.input_tokens, 3);
    assert_eq!(response.usage.output_tokens, 4);
}

#[test]
fn tool_result_round_trip_in_next_request() {
    let first = r#"{"content":[{"type":"tool_use","id":"tu_1","name":"read","input":{"path":"a"}}],"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let second = r#"{"content":[{"type":"text","text":"done"}],"usage":{"input_tokens":2,"output_tokens":1}}"#;
    let (url, requests) = scripted_server(vec![(200, first.into()), (200, second.into())]);
    let model = Anthropic::new(config(url)).unwrap();
    let first_response = model.complete(&tool_request()).unwrap();
    let mut follow_up = tool_request();
    follow_up
        .messages
        .push(Message::assistant("", first_response.tool_calls.clone()));
    follow_up
        .messages
        .push(Message::tool("file contents", "tu_1"));
    model.complete(&follow_up).unwrap();
    let captured = requests.lock().unwrap();
    let second_body = String::from_utf8_lossy(captured.last().unwrap());
    let json: serde_json::Value =
        serde_json::from_str(second_body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let messages = json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["content"][0]["type"], "tool_use");
    assert_eq!(messages[0]["content"][0]["id"], "tu_1");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"][0]["type"], "tool_result");
    assert_eq!(messages[1]["content"][0]["tool_use_id"], "tu_1");
    assert_eq!(messages[1]["content"][0]["content"], "file contents");
}

#[test]
fn tool_error_sets_is_error() {
    let model = Anthropic::new(config("http://localhost".into())).unwrap();
    let mut req = tool_request();
    req.messages.push(Message::assistant(
        "",
        vec![ToolCall {
            id: "tu_1".into(),
            name: "read".into(),
            arguments: "{}".into(),
            thought_signature: None,
        }],
    ));
    req.messages.push(Message::tool("Error: not found", "tu_1"));
    let body = model.request_body(&req, None).unwrap();
    assert_eq!(body["messages"][1]["content"][0]["is_error"], true);
}

#[test]
fn unauthorized_redacts_key() {
    const KEY: &str = "anthropic-secret-key-77";
    std::env::set_var("M20_ANTHROPIC_401_KEY", KEY);
    let (url, _) = scripted_server(vec![(401, format!("bad {KEY}"))]);
    let mut cfg = config(url);
    cfg.api_key_env = "M20_ANTHROPIC_401_KEY".into();
    let err = Anthropic::new(cfg)
        .unwrap()
        .complete(&tool_request())
        .unwrap_err()
        .to_string();
    assert!(!err.contains(KEY));
    assert!(err.contains("[REDACTED]"));
    std::env::remove_var("M20_ANTHROPIC_401_KEY");
}

#[test]
fn missing_message_content_is_format_error() {
    let (url, _) = scripted_server(vec![(
        200,
        r#"{"usage":{"input_tokens":0,"output_tokens":0}}"#.into(),
    )]);
    assert!(matches!(
        Anthropic::new(config(url))
            .unwrap()
            .complete(&tool_request()),
        Err(ModelError::FormatError(_))
    ));
    let (url, _) = scripted_server(vec![(200, r#"{"content":[]}"#.into())]);
    assert!(matches!(
        Anthropic::new(config(url))
            .unwrap()
            .complete(&tool_request()),
        Err(ModelError::FormatError(_))
    ));
}

#[test]
fn base_url_with_v1_suffix_does_not_double() {
    let reply = r#"{"content":[{"type":"text","text":"ok"}]}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into())]);
    let mut cfg = config(url);
    cfg.base_url = format!("{}/v1", cfg.base_url);
    Anthropic::new(cfg)
        .unwrap()
        .complete(&tool_request())
        .unwrap();
    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&captured[0]);
    assert!(raw.contains("POST /v1/messages "));
    assert!(!raw.contains("POST /v1/v1/messages "));
}

#[test]
fn request_body_uses_object_tool_choice_and_omits_when_no_tools() {
    let model = Anthropic::new(config("http://localhost".into())).unwrap();
    let with_tools = model.request_body(&tool_request(), None).unwrap();
    assert_eq!(
        with_tools["tool_choice"],
        serde_json::json!({"type": "auto"})
    );
    assert!(with_tools["tools"].as_array().is_some_and(|a| a.len() == 1));

    let no_tools_req = Request {
        system_prompt: String::new(),
        messages: vec![Message::user("hi")],
        tools: vec![],
    };
    let without_tools = model.request_body(&no_tools_req, None).unwrap();
    assert!(without_tools.get("tools").is_none(), "{without_tools}");
    assert!(
        without_tools.get("tool_choice").is_none(),
        "{without_tools}"
    );
}

#[test]
fn system_prompt_is_sent_as_is_with_api_key_auth() {
    let reply =
        r#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into())]);
    std::env::set_var("M20_ANTHROPIC_SYS_KEY", "sk-ant-oat-test-token-123");
    let mut cfg = config(url);
    cfg.api_key_env = "M20_ANTHROPIC_SYS_KEY".into();
    let req = Request {
        system_prompt: "repo rules".into(),
        messages: vec![Message::user("hi")],
        tools: vec![],
    };
    Anthropic::new(cfg).unwrap().complete(&req).unwrap();
    std::env::remove_var("M20_ANTHROPIC_SYS_KEY");

    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&captured[0]);
    let headers = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
    assert!(headers.contains("x-api-key: sk-ant-oat-test-token-123"));
    assert!(!headers.contains("authorization:"));
    assert!(!headers.contains("anthropic-beta:"));
    let body: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["system"], "repo rules");
}

#[test]
fn reasoning_effort_maps_to_anthropic_thinking_and_interleaved_beta() {
    let reply =
        r#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let (url, requests) = scripted_server(vec![(200, reply.into())]);
    let mut cfg = config(url);
    cfg.extra_body
        .insert("reasoning_effort".into(), serde_json::json!("medium"));
    Anthropic::new(cfg)
        .unwrap()
        .complete(&tool_request())
        .unwrap();
    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&captured[0]);
    let headers = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
    assert!(headers.contains("anthropic-beta: interleaved-thinking-2025-05-14"));
    let body: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(body.get("reasoning_effort").is_none(), "{body}");
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "enabled", "budget_tokens": 4096})
    );
    assert_eq!(body["max_tokens"], 8192);
}

#[test]
fn thinking_blocks_replay_ahead_of_tool_use_only_when_effort_set() {
    let first = r#"{"content":[{"type":"thinking","thinking":"plan","signature":"SIG-1"},{"type":"redacted_thinking","data":"ENC"},{"type":"tool_use","id":"tu_1","name":"read","input":{"path":"a"}}],"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let second = r#"{"content":[{"type":"text","text":"done"}],"usage":{"input_tokens":2,"output_tokens":1}}"#;
    let (url, requests) = scripted_server(vec![(200, first.into()), (200, second.into())]);
    let mut cfg = config(url.clone());
    cfg.extra_body
        .insert("reasoning_effort".into(), serde_json::json!("low"));
    let model = Anthropic::new(cfg).unwrap();
    let first_response = model.complete(&tool_request()).unwrap();
    let mut follow_up = tool_request();
    follow_up
        .messages
        .push(Message::assistant("", first_response.tool_calls.clone()));
    follow_up
        .messages
        .push(Message::tool("file contents", "tu_1"));
    model.complete(&follow_up).unwrap();
    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(captured.last().unwrap());
    let json: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let content = &json["messages"][0]["content"];
    assert_eq!(
        content[0],
        serde_json::json!({"type": "thinking", "thinking": "plan", "signature": "SIG-1"})
    );
    assert_eq!(
        content[1],
        serde_json::json!({"type": "redacted_thinking", "data": "ENC"})
    );
    assert_eq!(content[2]["type"], "tool_use");
    assert_eq!(content.as_array().unwrap().len(), 3);

    let plain = Anthropic::new(config(url)).unwrap();
    let body = plain.request_body(&follow_up, None).unwrap();
    assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
    assert!(body.get("thinking").is_none());
}
