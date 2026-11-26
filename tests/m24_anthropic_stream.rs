use devcode::agent::anthropic::Anthropic;
use devcode::agent::model::{Message, Model, ModelError, Request, ToolSchema};
use devcode::config::{ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn config(url: String) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::Anthropic,
        base_url: url,
        api_key_env: String::new(),
        connector: None,
        model: "claude-test".into(),
        context_window: 100_000,
        max_rpm: None,
        extra_body: Default::default(),
    }
}

fn text_request() -> Request {
    Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
    }
}

fn tool_request() -> Request {
    Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![ToolSchema {
            name: "read".into(),
            description: "read a file".into(),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        }],
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
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
    if length > 0 {
        stream.read_exact(&mut data[header_end..]).unwrap();
    }
    data
}

fn request_json(raw: &[u8]) -> Value {
    let text = String::from_utf8_lossy(raw);
    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn sse(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn write_sse_headers(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    stream.flush().unwrap();
}

fn write_chunk(stream: &mut TcpStream, payload: &str) {
    write!(stream, "{:x}\r\n{payload}\r\n", payload.len()).unwrap();
    stream.flush().unwrap();
}

fn end_chunks(stream: &mut TcpStream) {
    stream.write_all(b"0\r\n\r\n").unwrap();
    stream.flush().unwrap();
    let _ = stream.shutdown(Shutdown::Write);
}

fn write_fixed(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.flush().unwrap();
}

fn spawn_sse(events: Vec<String>) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nodelay(true).unwrap();
        let raw = read_http_request(&mut stream);
        recorded.lock().unwrap().push(raw);
        write_sse_headers(&mut stream);
        for event in &events {
            write_chunk(&mut stream, event);
        }
        end_chunks(&mut stream);
    });
    (format!("http://{address}"), requests)
}

fn stream_text(model: &Anthropic, req: &Request) -> (devcode::agent::model::Response, Vec<String>) {
    let parts = Mutex::new(Vec::new());
    let response = model
        .complete_stream(req, &mut |delta| {
            parts.lock().unwrap().push(delta.to_owned())
        })
        .unwrap();
    (response, parts.into_inner().unwrap())
}

#[test]
fn stream_usage_and_text_deltas() {
    let events = vec![
        sse("ping", json!({"type": "ping"})),
        sse(
            "message_start",
            json!({
                "type": "message_start",
                "message": {"usage": {"input_tokens": 12, "output_tokens": 99}}
            }),
        ),
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hello"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": " "}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "world"}
            }),
        ),
        sse(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        sse(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 4}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, requests) = spawn_sse(events);
    let (response, parts) = stream_text(&Anthropic::new(config(url)).unwrap(), &text_request());
    assert_eq!(
        parts,
        vec!["Hello".to_string(), " ".to_string(), "world".to_string()]
    );
    assert_eq!(response.text.as_deref(), Some("Hello world"));
    assert_eq!(response.usage.input_tokens, Some(12));
    assert_eq!(response.usage.output_tokens, Some(4));
    let captured = requests.lock().unwrap();
    let raw = String::from_utf8_lossy(&captured[0]);
    let headers = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
    assert!(headers.contains("anthropic-version: 2023-06-01"));
    assert!(!headers.contains("anthropic-beta"));
    assert!(raw.contains("POST /v1/messages "));
    assert_eq!(request_json(raw.as_bytes())["stream"], true);
}

#[test]
fn stream_tool_partial_json_assembled() {
    let events = vec![
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "tool_use", "id": "tu_b", "name": "read"}
            }),
        ),
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "tu_a", "name": "read"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\": "}
            }),
        ),
        sse("ping", json!({"type": "ping"})),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "\"a.rs\"}"}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, _) = spawn_sse(events);
    let (response, parts) = stream_text(&Anthropic::new(config(url)).unwrap(), &tool_request());
    assert!(parts.is_empty());
    assert!(response.text.is_none());
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].id, "tu_a");
    assert_eq!(response.tool_calls[0].name, "read");
    assert_eq!(response.tool_calls[0].arguments, "{\"path\": \"a.rs\"}");
    assert_eq!(response.tool_calls[1].id, "tu_b");
    assert_eq!(response.tool_calls[1].name, "read");
    assert_eq!(response.tool_calls[1].arguments, "{}");
}

#[test]
fn stream_ignores_thinking_delta() {
    let events = vec![
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "HIDDEN-THOUGHT"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "signature_delta", "signature": "SIG-BLOB"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "text_delta", "text": "visible"}
            }),
        ),
    ];
    let (url, _) = spawn_sse(events);
    let (response, parts) = stream_text(&Anthropic::new(config(url)).unwrap(), &text_request());
    assert_eq!(response.text.as_deref(), Some("visible"));
    assert_eq!(parts.concat(), "visible");
    let joined = parts.join("");
    assert!(!joined.contains("HIDDEN-THOUGHT"));
    assert!(!joined.contains("SIG-BLOB"));
    assert!(!response.text.unwrap().contains("HIDDEN-THOUGHT"));
}

#[test]
fn stream_unknown_tool_name_is_format_error() {
    let events = vec![
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "tu_x", "name": "wipe"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{}"}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, _) = spawn_sse(events);
    let err = Anthropic::new(config(url))
        .unwrap()
        .complete_stream(&tool_request(), &mut |_| ())
        .unwrap_err();
    match err {
        ModelError::FormatError(msg) => assert_eq!(msg, "unknown tool name"),
        other => panic!("expected format error, got {other}"),
    }
}

#[test]
fn stream_redacts_key_split_across_deltas() {
    const KEY: &str = "m24StreamSecret";
    let cut1 = "aa m24Str";
    let cut2 = "eamSecret bb m24Stream";
    let cut3 = "Secret cc";
    let raw = format!("{cut1}{cut2}{cut3}");
    assert_eq!(raw, format!("aa {KEY} bb {KEY} cc"));
    let events = vec![
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": cut1}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": cut2}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": cut3}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, requests) = spawn_sse(events);
    std::env::set_var("M24_ANTHROPIC_STREAM_SPLIT_KEY", KEY);
    let mut cfg = config(url);
    cfg.api_key_env = "M24_ANTHROPIC_STREAM_SPLIT_KEY".into();
    let model = Anthropic::new(cfg).unwrap();
    let (response, parts) = stream_text(&model, &text_request());
    assert_eq!(response.text.as_deref(), Some(raw.as_str()));
    assert!(response.text.unwrap().contains(KEY));
    assert_eq!(parts.concat(), raw.replace(KEY, "[REDACTED]"));
    for part in &parts {
        assert!(!part.contains(KEY), "callback leaked secret: {part:?}");
    }
    let captured = requests.lock().unwrap();
    let raw_request = String::from_utf8_lossy(&captured[0]);
    let headers = raw_request
        .split("\r\n\r\n")
        .next()
        .unwrap()
        .to_ascii_lowercase();
    assert!(headers.contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase())));
    assert!(!headers.contains("anthropic-beta"));
    std::env::remove_var("M24_ANTHROPIC_STREAM_SPLIT_KEY");
}

#[test]
fn stream_delivers_before_rest_of_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let arrived = Arc::new(AtomicBool::new(false));
    let first = Arc::new(Mutex::new(String::new()));
    let arrived_flag = Arc::clone(&arrived);
    let first_text = Arc::clone(&first);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nodelay(true).unwrap();
        let _ = read_http_request(&mut stream);
        write_sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            &sse(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "LEFT"}
                }),
            ),
        );
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(text) => {
                *first_text.lock().unwrap() = text;
                arrived_flag.store(true, Ordering::SeqCst);
                write_chunk(
                    &mut stream,
                    &sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "text_delta", "text": "RIGHT"}
                        }),
                    ),
                );
                write_chunk(
                    &mut stream,
                    &sse("message_stop", json!({"type": "message_stop"})),
                );
                end_chunks(&mut stream);
            }
            Err(_) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    });
    let model = Anthropic::new(config(format!("http://{address}"))).unwrap();
    let sent = Mutex::new(Some(tx));
    let parts = Mutex::new(Vec::new());
    let result = model.complete_stream(&text_request(), &mut |delta| {
        if let Some(tx) = sent.lock().unwrap().take() {
            tx.send(delta.to_owned()).unwrap();
        }
        parts.lock().unwrap().push(delta.to_owned());
    });
    server.join().unwrap();
    assert!(
        arrived.load(Ordering::SeqCst),
        "first callback did not arrive within 2s"
    );
    assert_eq!(first.lock().unwrap().as_str(), "LEFT");
    let response = result.expect("incremental stream");
    assert_eq!(
        parts.into_inner().unwrap(),
        vec!["LEFT".to_string(), "RIGHT".to_string()]
    );
    assert_eq!(response.text.as_deref(), Some("LEFTRIGHT"));
}

#[test]
fn stream_socket_close_is_error_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let counted = Arc::clone(&connections);
    let stop_flag = Arc::clone(&stop);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        counted.fetch_add(1, Ordering::SeqCst);
        stream.set_nodelay(true).unwrap();
        let _ = read_http_request(&mut stream);
        write_sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            "event: content_block_delta\ndata: {\"type\":\"content_block_del",
        );
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let mut grace: Option<Instant> = None;
        loop {
            if stop_flag.load(Ordering::SeqCst) && grace.is_none() {
                grace = Some(Instant::now() + Duration::from_millis(200));
            }
            if grace.is_some_and(|deadline| Instant::now() >= deadline) {
                break;
            }
            match listener.accept() {
                Ok((extra, _)) => {
                    counted.fetch_add(1, Ordering::SeqCst);
                    drop(extra);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(15));
                }
                Err(_) => break,
            }
        }
    });
    let model = Anthropic::new(config(format!("http://{address}"))).unwrap();
    let result = model.complete_stream(&text_request(), &mut |_| ());
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    assert!(result.is_err(), "closed socket returned {result:?}");
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[test]
fn complete_still_sends_stream_false() {
    let reply =
        r#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":2}}"#;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let body = reply.to_owned();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let raw = read_http_request(&mut stream);
        recorded.lock().unwrap().push(raw);
        write_fixed(&mut stream, 200, "application/json", &body);
        let _ = stream.shutdown(Shutdown::Write);
    });
    Anthropic::new(config(format!("http://{address}")))
        .unwrap()
        .complete(&text_request())
        .unwrap();
    let captured = requests.lock().unwrap();
    assert_eq!(request_json(&captured[0])["stream"], false);
}

#[test]
fn stream_json_content_type_skips_callbacks() {
    let body = r#"{"content":[{"type":"text","text":"plain"}],"usage":{"input_tokens":2,"output_tokens":3}}"#;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let payload = body.to_owned();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut stream);
        write_fixed(&mut stream, 200, "application/json", &payload);
        let _ = stream.shutdown(Shutdown::Write);
    });
    let calls = Mutex::new(0u32);
    let response = Anthropic::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream(&text_request(), &mut |_| {
            *calls.lock().unwrap() += 1;
        })
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 0);
    assert_eq!(response.text.as_deref(), Some("plain"));
    assert_eq!(response.usage.input_tokens, Some(2));
    assert_eq!(response.usage.output_tokens, Some(3));
}

#[test]
fn stream_http_400_falls_back_once() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let raw = read_http_request(&mut stream);
            let streamed = request_json(&raw)["stream"] == json!(true);
            recorded.lock().unwrap().push(raw);
            if streamed {
                write_fixed(&mut stream, 400, "application/json", "no stream");
            } else {
                write_fixed(
                    &mut stream,
                    200,
                    "application/json",
                    r#"{"content":[{"type":"text","text":"fallback"}],"usage":{"input_tokens":1,"output_tokens":1}}"#,
                );
            }
            let _ = stream.shutdown(Shutdown::Write);
        }
    });
    let calls = Mutex::new(0u32);
    let response = Anthropic::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream(&text_request(), &mut |_| {
            *calls.lock().unwrap() += 1;
        })
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), 0);
    assert_eq!(response.text.as_deref(), Some("fallback"));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(request_json(&captured[0])["stream"], true);
    assert_eq!(request_json(&captured[1])["stream"], false);
}

#[test]
fn stream_emits_thinking_deltas_as_reasoning_chunks() {
    use devcode::agent::StreamChunk;
    const KEY: &str = "ant-think-secret";
    std::env::set_var("M24_ANTHROPIC_REASON_KEY", KEY);
    let events = vec![
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "plan ant-think-"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "secret ok"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "text_delta", "text": "visible"}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, _) = spawn_sse(events);
    let mut cfg = config(url);
    cfg.api_key_env = "M24_ANTHROPIC_REASON_KEY".into();
    let mut text = String::new();
    let mut reasoning = String::new();
    let response = Anthropic::new(cfg)
        .unwrap()
        .complete_stream_cancellable(&text_request(), None, &mut |chunk| match chunk {
            StreamChunk::Text(s) => text.push_str(s),
            StreamChunk::Reasoning(s) => {
                assert!(!s.contains(KEY), "key leaked in reasoning {s:?}");
                reasoning.push_str(s);
            }
        })
        .unwrap();
    std::env::remove_var("M24_ANTHROPIC_REASON_KEY");
    assert_eq!(text, "visible");
    assert_eq!(response.text.as_deref(), Some("visible"));
    assert_eq!(reasoning, "plan [REDACTED] ok");
}

#[test]
fn cancel_aborts_stalled_anthropic_stream_immediately() {
    use devcode::agent::CancelToken;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut stream);
        write_sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            &sse(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "first"}
                }),
            ),
        );
        let _ = ready_tx.send(());
        thread::sleep(Duration::from_secs(5));
    });
    let cancel = CancelToken::new();
    let cancel_thread = cancel.clone();
    thread::spawn(move || {
        let _ = ready_rx.recv();
        thread::sleep(Duration::from_millis(20));
        cancel_thread.cancel();
    });
    let start = Instant::now();
    let err = Anthropic::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream_cancellable(&text_request(), Some(&cancel), &mut |_| {})
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(matches!(err, ModelError::Cancelled), "got {err}");
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn stream_keeps_thinking_signature_for_tool_turn_replay() {
    let events = vec![
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": ""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "step "}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "one"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "signature_delta", "signature": "SIG-BLOB"}
            }),
        ),
        sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "tool_use", "id": "tu_a", "name": "read"}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "{}"}
            }),
        ),
        sse("message_stop", json!({"type": "message_stop"})),
    ];
    let (url, _) = spawn_sse(events);
    let mut cfg = config(url);
    cfg.extra_body
        .insert("reasoning_effort".into(), json!("high"));
    let model = Anthropic::new(cfg).unwrap();
    let (response, _) = stream_text(&model, &tool_request());
    assert_eq!(response.tool_calls.len(), 1);
    let mut follow_up = tool_request();
    follow_up
        .messages
        .push(Message::assistant("", response.tool_calls));
    follow_up.messages.push(Message::tool("ok", "tu_a"));
    let body = model.request_body(&follow_up, None).unwrap();
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({"type": "thinking", "thinking": "step one", "signature": "SIG-BLOB"})
    );
    assert_eq!(body["messages"][0]["content"][1]["type"], "tool_use");
}
