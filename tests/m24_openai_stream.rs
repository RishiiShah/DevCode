use devcode::agent::model::{Model, ModelError, Request, ToolSchema};
use devcode::agent::openai_compat::OpenAiCompat;
use devcode::config::{ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
        tools: vec![
            ToolSchema {
                name: "read".into(),
                description: "read file".into(),
                parameters: json!({"type": "object"}),
            },
            ToolSchema {
                name: "write".into(),
                description: "write file".into(),
                parameters: json!({"type": "object"}),
            },
        ],
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.set_nodelay(true).unwrap();
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
    let body = text.split("\r\n\r\n").nth(1).unwrap();
    serde_json::from_str(body).unwrap()
}

fn write_chunk(stream: &mut TcpStream, data: &str) {
    stream
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .unwrap();
    stream.write_all(data.as_bytes()).unwrap();
    stream.write_all(b"\r\n").unwrap();
    stream.flush().unwrap();
}

fn sse_headers(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    stream.flush().unwrap();
}

fn data_event(value: Value) -> String {
    format!("data: {value}\n\n")
}

fn done_event() -> String {
    "data: [DONE]\n\n".into()
}

fn write_bytes(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

enum Reply {
    Bytes {
        status: u16,
        content_type: &'static str,
        body: String,
    },
    /// Chunked `text/event-stream`. Each event is flushed on its own.
    Sse(Vec<String>),
}

fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let raw = read_request(&mut stream);
            recorded.lock().unwrap().push(raw);
            match reply {
                Reply::Bytes {
                    status,
                    content_type,
                    body,
                } => write_bytes(&mut stream, status, content_type, &body),
                Reply::Sse(events) => {
                    sse_headers(&mut stream);
                    for event in &events {
                        write_chunk(&mut stream, event);
                    }
                    stream.write_all(b"0\r\n\r\n").unwrap();
                    let _ = stream.flush();
                    let _ = stream.shutdown(Shutdown::Write);
                }
            }
        }
    });
    (format!("http://{address}"), requests)
}

fn hello_events() -> Vec<String> {
    vec![
        data_event(
            json!({"choices":[{"delta":{"content":"Hel","reasoning_content":"hidden-reason"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}),
        ),
        data_event(json!({"choices":[{"delta":{"reasoning_content":"still-hidden"}}]})),
        data_event(json!({"choices":[{"delta":{"content":"lo"}}]})),
        data_event(
            json!({"choices":[{"delta":{}}],"usage":{"prompt_tokens":11,"completion_tokens":4}}),
        ),
        done_event(),
    ]
}

#[test]
fn stream_request_text_deltas_and_usage() {
    let (url, requests) = serve(vec![Reply::Sse(hello_events())]);
    let mut parts = Vec::new();
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |delta| parts.push(delta.to_owned()))
        .unwrap();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let raw = String::from_utf8_lossy(&sent[0]);
    assert!(raw.starts_with("POST /chat/completions "), "{raw}");
    let body = request_json(&sent[0]);
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(parts.concat(), "Hello");
    assert!(!parts.iter().any(|part| part.contains("hidden")));
    assert_eq!(response.text.as_deref(), Some("Hello"));
    assert_eq!(response.usage.input_tokens, 11);
    assert_eq!(response.usage.output_tokens, 4);
}

#[test]
fn tool_calls_assemble_in_index_order() {
    let args_a = r#"{"pa"#;
    let args_b = r#"th":"b"}"#;
    assert_eq!(format!("{args_a}{args_b}"), r#"{"path":"b"}"#);
    let events = vec![
        data_event(json!({"choices":[{"delta":{"tool_calls":[{
            "index": 1,
            "function": {"name": "wri", "arguments": args_a}
        }]}}]})),
        data_event(json!({"choices":[{"delta":{"tool_calls":[{
            "index": 0,
            "id": "call_a",
            "extra_content": {"google": {"thought_signature": "sig-abc"}},
            "function": {"name": "read", "arguments": "{"}
        }]}}]})),
        data_event(json!({"choices":[{"delta":{"tool_calls":[{
            "index": 1,
            "id": "call_b",
            "function": {"name": "te", "arguments": args_b}
        }]}}]})),
        data_event(json!({"choices":[{"delta":{"tool_calls":[{
            "index": 0,
            "function": {"arguments": "}"}
        }]}}]})),
        done_event(),
    ];
    let (url, _) = serve(vec![Reply::Sse(events)]);
    let mut parts = Vec::new();
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |delta| parts.push(delta.to_owned()))
        .unwrap();
    assert!(parts.is_empty());
    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].id, "call_a");
    assert_eq!(response.tool_calls[0].name, "read");
    assert_eq!(response.tool_calls[0].arguments, "{}");
    assert_eq!(
        response.tool_calls[0].thought_signature.as_deref(),
        Some("sig-abc")
    );
    assert_eq!(response.tool_calls[1].id, "call_b");
    assert_eq!(response.tool_calls[1].name, "write");
    assert_eq!(response.tool_calls[1].arguments, r#"{"path":"b"}"#);
    assert!(response.text.is_none());
}

#[test]
fn split_key_is_redacted_in_callbacks() {
    const KEY: &str = "sk-stream-secret";
    std::env::set_var("M24_OPENAI_STREAM_SPLIT_KEY", KEY);
    let events = vec![
        data_event(json!({"choices":[{"delta":{"content":"aa sk-stream-"}}]})),
        data_event(json!({"choices":[{"delta":{"content":"secret bb"}}]})),
        done_event(),
    ];
    let (url, _) = serve(vec![Reply::Sse(events)]);
    let mut cfg = config(url);
    cfg.api_key_env = "M24_OPENAI_STREAM_SPLIT_KEY".into();
    let mut parts = Vec::new();
    let response = OpenAiCompat::new(cfg)
        .unwrap()
        .complete_stream(&request(), &mut |delta| {
            assert!(!delta.contains(KEY), "key leaked in callback {delta:?}");
            parts.push(delta.to_owned());
        })
        .unwrap();
    std::env::remove_var("M24_OPENAI_STREAM_SPLIT_KEY");
    let joined = parts.concat();
    assert_eq!(joined, "aa [REDACTED] bb");
    assert!(!joined.contains(KEY));
    assert_eq!(response.text.as_deref(), Some("aa sk-stream-secret bb"));
}

#[test]
fn retry_429_then_stream() {
    let (url, requests) = serve(vec![
        Reply::Bytes {
            status: 429,
            content_type: "text/plain",
            body: "busy".into(),
        },
        Reply::Sse(hello_events()),
    ]);
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |_| {})
        .unwrap();
    assert_eq!(response.text.as_deref(), Some("Hello"));
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(request_json(&sent[0])["stream"], true);
    assert_eq!(request_json(&sent[1])["stream"], true);
}

#[test]
fn socket_closed_after_first_event_is_one_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&done);
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        counted.fetch_add(1, Ordering::SeqCst);
        let _ = read_request(&mut stream);
        sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            &data_event(json!({"choices":[{"delta":{"content":"Hi"}}]})),
        );
        let _ = stream.shutdown(Shutdown::Write);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((extra, _)) => {
                    counted.fetch_add(1, Ordering::SeqCst);
                    drop(extra);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    let err = OpenAiCompat::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream(&request(), &mut |_| {})
        .unwrap_err();
    done.store(true, Ordering::SeqCst);
    assert!(
        matches!(err, ModelError::Network(_) | ModelError::FormatError(_)),
        "{err}"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[test]
fn http_400_falls_back_to_json_complete() {
    let (url, requests) = serve(vec![
        Reply::Bytes {
            status: 400,
            content_type: "application/json",
            body: r#"{"error":"stream unsupported"}"#.into(),
        },
        Reply::Bytes {
            status: 200,
            content_type: "application/json",
            body: r#"{"choices":[{"message":{"content":"fallback"}}]}"#.into(),
        },
    ]);
    let mut calls = 0;
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |_| calls += 1)
        .unwrap();
    assert_eq!(calls, 0);
    assert_eq!(response.text.as_deref(), Some("fallback"));
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(request_json(&sent[0])["stream"], true);
    assert_eq!(request_json(&sent[1])["stream"], false);
}

#[test]
fn deltas_arrive_before_the_rest_of_the_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (notify_tx, notify_rx) = mpsc::channel::<()>();
    let (result_tx, result_rx) = mpsc::channel::<bool>();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            &data_event(json!({"choices":[{"delta":{"content":"Hel"}}]})),
        );
        let signaled = notify_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        let _ = result_tx.send(signaled);
        if !signaled {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        write_chunk(
            &mut stream,
            &data_event(json!({"choices":[{"delta":{"content":"lo"}}]})),
        );
        write_chunk(
            &mut stream,
            &data_event(
                json!({"choices":[{"delta":{}}],"usage":{"prompt_tokens":2,"completion_tokens":3}}),
            ),
        );
        write_chunk(&mut stream, &done_event());
        stream.write_all(b"0\r\n\r\n").unwrap();
        let _ = stream.flush();
        let _ = stream.shutdown(Shutdown::Write);
    });
    let mut parts = Vec::new();
    let response = OpenAiCompat::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream(&request(), &mut |delta| {
            parts.push(delta.to_owned());
            let _ = notify_tx.send(());
        })
        .expect("incremental callback should unblock the rest of the body");
    assert!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "server did not see a callback before sending the rest"
    );
    assert_eq!(parts.concat(), "Hello");
    assert_eq!(response.text.as_deref(), Some("Hello"));
    assert_eq!(response.usage.input_tokens, 2);
    assert_eq!(response.usage.output_tokens, 3);
}

#[test]
fn non_sse_json_200_skips_callbacks() {
    let (url, requests) = serve(vec![Reply::Bytes {
        status: 200,
        content_type: "application/json",
        body: r#"{"choices":[{"message":{"content":"json-body","reasoning_content":"hidden"}}],"usage":{"prompt_tokens":3,"completion_tokens":5}}"#.into(),
    }]);
    let mut calls = 0;
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |_| calls += 1)
        .unwrap();
    assert_eq!(calls, 0);
    assert_eq!(response.text.as_deref(), Some("json-body"));
    assert_eq!(response.usage.input_tokens, 3);
    assert_eq!(response.usage.output_tokens, 5);
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(request_json(&sent[0])["stream"], true);
    assert_eq!(
        request_json(&sent[0])["stream_options"]["include_usage"],
        true
    );
}

#[test]
fn empty_streamed_tool_arguments_become_empty_object() {
    let events = vec![
        data_event(json!({"choices":[{"delta":{"tool_calls":[{
            "index": 0,
            "id": "call_a",
            "function": {"name": "read", "arguments": ""}
        }]}}]})),
        done_event(),
    ];
    let (url, _) = serve(vec![Reply::Sse(events)]);
    let response = OpenAiCompat::new(config(url))
        .unwrap()
        .complete_stream(&request(), &mut |_| {})
        .unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].arguments, "{}");
}

#[test]
fn stream_emits_reasoning_chunks_and_redacts_keys() {
    use devcode::agent::StreamChunk;
    const KEY: &str = "sk-reason-secret";
    std::env::set_var("M24_OPENAI_REASON_KEY", KEY);
    let events = vec![
        data_event(json!({"choices":[{"delta":{"reasoning_content":"think sk-reason-"}}]})),
        data_event(json!({"choices":[{"delta":{"reasoning":"secret!"}}]})),
        data_event(json!({"choices":[{"delta":{"content":"answer"}}]})),
        done_event(),
    ];
    let (url, _) = serve(vec![Reply::Sse(events)]);
    let mut cfg = config(url);
    cfg.api_key_env = "M24_OPENAI_REASON_KEY".into();
    let mut text = String::new();
    let mut reasoning = String::new();
    let response = OpenAiCompat::new(cfg)
        .unwrap()
        .complete_stream_cancellable(&request(), None, &mut |chunk| match chunk {
            StreamChunk::Text(s) => text.push_str(s),
            StreamChunk::Reasoning(s) => {
                assert!(!s.contains(KEY), "key leaked in reasoning chunk {s:?}");
                reasoning.push_str(s);
            }
        })
        .unwrap();
    std::env::remove_var("M24_OPENAI_REASON_KEY");
    assert_eq!(text, "answer");
    assert_eq!(response.text.as_deref(), Some("answer"));
    assert_eq!(reasoning, "think [REDACTED]!");
}

#[test]
fn cancel_aborts_stalled_stream_immediately() {
    use devcode::agent::CancelToken;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_request(&mut stream);
        sse_headers(&mut stream);
        write_chunk(
            &mut stream,
            &data_event(json!({"choices":[{"delta":{"content":"partial"}}]})),
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
    let start = std::time::Instant::now();
    let err = OpenAiCompat::new(config(format!("http://{address}")))
        .unwrap()
        .complete_stream_cancellable(&request(), Some(&cancel), &mut |_| {})
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(matches!(err, ModelError::Cancelled), "got {err}");
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn rejected_stream_usage_is_disabled_for_later_requests() {
    let (url, requests) = serve(vec![
        Reply::Bytes {
            status: 400,
            content_type: "application/json",
            body: r#"{"error":{"param":"stream_options","message":"unsupported"}}"#.into(),
        },
        Reply::Sse(hello_events()),
        Reply::Sse(hello_events()),
    ]);
    let model = devcode::agent::session::RateLimitedModel::new(
        Box::new(OpenAiCompat::new(config(url)).unwrap()),
        Duration::ZERO,
    );
    for _ in 0..2 {
        assert_eq!(
            model
                .complete_stream(&request(), &mut |_| {})
                .unwrap()
                .text
                .as_deref(),
            Some("Hello")
        );
    }
    assert!(model.stream_usage_rejected());
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(
        request_json(&sent[0])["stream_options"]["include_usage"],
        true
    );
    for raw in &sent[1..] {
        let body = request_json(raw);
        assert_eq!(body["stream"], true);
        assert!(body.get("stream_options").is_none());
    }
}
