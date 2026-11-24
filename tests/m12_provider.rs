use devcode::agent::openai_compat::OpenAiCompat;
use devcode::agent::{
    run_agent_with_config, LoopLimits, Model, Request, Response, ScriptedModel, Usage,
};
use devcode::config::{Config, ProviderConfig, ProviderKind};
use devcode::credentials::save;
use devcode::Profile;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

fn server(key: &str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (send, recv) = mpsc::channel();
    let body = format!("bad {}", key);
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut b = [0];
            stream.read_exact(&mut b).unwrap();
            bytes.push(b[0]);
            if bytes.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let headers = String::from_utf8(bytes).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
            })
            .unwrap();
        let mut request_body = vec![0; length];
        stream.read_exact(&mut request_body).unwrap();
        send.send(headers).unwrap();
        write!(
            stream,
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    (url, recv)
}
fn config(url: String) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: url,
        api_key_env: "M12_FAKE_PROVIDER_KEY".into(),
        connector: Some("nim".into()),
        model: "fake".into(),
        context_window: 1000,
        max_rpm: None,
        extra_body: Default::default(),
    }
}
fn request() -> Request {
    Request {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
    }
}
#[test]
fn resolution_and_401_redaction_and_trace() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("config");
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    std::env::set_var("DEVCODE_CONFIG_DIR", &config_dir);
    let stored = "stored-fake-secret-123";
    let environment = "env-fake-secret-456";
    let mut entries = BTreeMap::new();
    entries.insert("nim".into(), stored.into());
    save(&config_dir, &entries).unwrap();
    std::env::set_var("M12_FAKE_PROVIDER_KEY", environment);
    let (url, headers) = server(environment);
    let error = OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .unwrap_err()
        .to_string();
    assert!(!error.contains(environment));
    assert!(error.contains("[REDACTED]"));
    assert!(headers
        .recv()
        .unwrap()
        .contains(&format!("Bearer {}", environment)));
    std::env::remove_var("M12_FAKE_PROVIDER_KEY");
    let (url, headers) = server(stored);
    let error = OpenAiCompat::new(config(url))
        .unwrap()
        .complete(&request())
        .unwrap_err()
        .to_string();
    assert!(!error.contains(stored));
    assert!(headers
        .recv()
        .unwrap()
        .contains(&format!("Bearer {}", stored)));
    save(&config_dir, &BTreeMap::new()).unwrap();
    let missing = match OpenAiCompat::new(config("http://localhost".into())) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("missing key accepted"),
    };
    assert!(missing.contains("devcode auth login nim"));
    assert!(missing.contains("M12_FAKE_PROVIDER_KEY"));
    let mut anonymous = config("http://localhost".into());
    anonymous.api_key_env.clear();
    anonymous.connector = None;
    assert!(OpenAiCompat::new(anonymous).is_ok());
    let model = ScriptedModel::new(vec![Ok(Response {
        text: Some("ok".into()),
        tool_calls: vec![],
        usage: Usage::default(),
    })]);
    let result = run_agent_with_config(
        Profile::Ask,
        &model,
        &Default::default(),
        "safe",
        &LoopLimits {
            trace: true,
            ..Default::default()
        },
        &root,
        &Config::default(),
    )
    .unwrap();
    assert!(!format!("{:?}", result).contains(stored));
    let trace = std::fs::read_to_string(root.join(".devcode/trace.jsonl")).unwrap();
    assert!(!trace.contains(stored));
    assert!(!trace.contains(environment));
}
