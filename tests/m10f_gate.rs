use devcode::agent::openai_compat::OpenAiCompat;
use devcode::agent::{
    run_agent_with_config, LoopLimits, Model, ModelError, Request, Response, ScriptedModel,
    ToolCall, Usage,
};
use devcode::approval::ApprovalPrompt;
use devcode::config::{parse_config, Config};
use devcode::gate::{evaluate_gate, GateError, ToolLog};
use devcode::tools::{registry_with_context, MutationContext};
use devcode::Profile;
use serde_json::json;
use std::io::{self, BufRead, Cursor, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn gate(log: &ToolLog) -> Result<(), GateError> {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.verification.test_command = Some("test".into());
    evaluate_gate(
        root.path(),
        &cfg,
        log,
        &[(PathBuf::from("src/a.rs"), "old".into(), "new".into())],
        false,
        None,
        "hash",
    )
    .map(|_| ())
}
#[test]
fn red_before_edit_green_after_allows() {
    let mut log = ToolLog::new();
    log.record_command("test", Some(1), 1);
    log.record_file_edit("src/a.rs", 2);
    log.record_command("test", Some(0), 3);
    assert!(gate(&log).is_ok());
}
#[test]
fn green_before_and_after_blocks() {
    let mut log = ToolLog::new();
    log.record_command("test", Some(0), 1);
    log.record_file_edit("src/a.rs", 2);
    log.record_command("test", Some(0), 3);
    assert!(matches!(
        gate(&log),
        Err(GateError::MissingTestAssertion(_))
    ));
}
#[test]
fn red_before_and_after_blocks() {
    let mut log = ToolLog::new();
    log.record_command("test", Some(1), 1);
    log.record_file_edit("src/a.rs", 2);
    log.record_command("test", Some(1), 3);
    assert!(matches!(gate(&log), Err(GateError::TestCommandFailed(1))));
}
#[test]
fn late_red_then_edit_then_green_allows() {
    let mut log = ToolLog::new();
    log.record_file_edit("src/a.rs", 1);
    log.record_command("test", Some(1), 2);
    log.record_file_edit("src/a.rs", 3);
    log.record_command("test", Some(0), 4);
    assert!(gate(&log).is_ok());
}
struct Inspect {
    request: Arc<Mutex<Option<Request>>>,
}
impl Model for Inspect {
    fn complete(&self, r: &Request) -> Result<Response, ModelError> {
        *self.request.lock().unwrap() = Some(r.clone());
        Ok(Response {
            text: Some("done".into()),
            tool_calls: vec![],
            usage: Usage::default(),
        })
    }
}
#[test]
fn build_request_contains_instructions() {
    let root = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let mut config = Config::default();
    config.verification.test_command = Some("cargo test".into());
    run_agent_with_config(
        Profile::Build,
        &Inspect {
            request: seen.clone(),
        },
        &Default::default(),
        "task",
        &LoopLimits::default(),
        root.path(),
        &config,
    )
    .unwrap();
    let req = seen.lock().unwrap().clone().unwrap();
    assert!(req.system_prompt.contains("run it before editing"));
    assert!(req
        .system_prompt
        .contains("declare_scope before the first edit"));
    assert!(req
        .system_prompt
        .contains("Do not add tests or assertions unless"));
    assert!(req.system_prompt.contains("cargo test"));
}
struct SlowReader(Cursor<Vec<u8>>);
impl Read for SlowReader {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.0.read(b)
    }
}
impl BufRead for SlowReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.0.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.0.consume(n)
    }
    fn read_line(&mut self, s: &mut String) -> io::Result<usize> {
        std::thread::sleep(Duration::from_millis(40));
        self.0.read_line(s)
    }
}
struct SlowModel(ScriptedModel);
impl Model for SlowModel {
    fn complete(&self, r: &Request) -> Result<Response, ModelError> {
        std::thread::sleep(Duration::from_millis(40));
        self.0.complete(r)
    }
}
#[test]
fn trace_separates_model_tool_and_approval() {
    let root = tempfile::tempdir().unwrap();
    let context = MutationContext::new(
        root.path().to_path_buf(),
        "test".into(),
        ApprovalPrompt::new(
            SlowReader(Cursor::new(b"y\n".to_vec())),
            Vec::<u8>::new(),
            true,
            false,
        ),
    );
    let tools = registry_with_context(context);
    let response = |calls| {
        Ok(Response {
            text: Some("done".into()),
            tool_calls: calls,
            usage: Usage::default(),
        })
    };
    let call = |name: &str, args: serde_json::Value| ToolCall {
        id: name.into(),
        thought_signature: None,
        name: name.into(),
        arguments: args.to_string(),
    };
    let model = SlowModel(ScriptedModel::new(vec![
        response(vec![
            call("declare_scope", json!({"files":["a"],"reason":"x"})),
            call("write_file", json!({"path":"a","content":" "})),
        ]),
        response(vec![]),
    ]));
    run_agent_with_config(
        Profile::Build,
        &model,
        &tools,
        "task",
        &LoopLimits {
            trace: true,
            ..Default::default()
        },
        root.path(),
        &Config::default(),
    )
    .unwrap();
    let trace = std::fs::read_to_string(root.path().join(".devcode/trace.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = trace
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0]["model_ms"].as_u64().unwrap() >= 35);
    assert!(rows[0]["approval_wait_ms"].as_u64().unwrap() >= 35);
    assert!(rows[0]["tool_ms"].as_u64().unwrap() < rows[0]["approval_wait_ms"].as_u64().unwrap());
    assert_eq!(rows[1]["tool_name"], "(final)");
    assert_eq!(rows[1]["tool_ms"], 0);
    assert!(rows[0].get("elapsed_ms").is_none());
}
#[test]
fn extra_body_merge_and_override_rejection() {
    let provider = |extra: serde_json::Value| {
        json!({"provider":{"kind":"openai_compat","base_url":"http://localhost","model":"m","context_window":100,"extra_body":extra}}).to_string()
    };
    let config = parse_config(&provider(
        json!({"chat_template_kwargs":{"enable_thinking":false}}),
    ))
    .unwrap()
    .provider
    .unwrap();
    let adapter = OpenAiCompat::new(config).unwrap();
    let body = adapter.request_body(
        &Request {
            system_prompt: "s".into(),
            messages: vec![],
            tools: vec![],
        },
        None,
    );
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(body["model"], "m");
    for key in ["model", "messages", "tools", "stream"] {
        let mut value = serde_json::Map::new();
        value.insert(key.into(), json!("bad"));
        assert!(parse_config(&provider(value.into())).is_err());
    }
    assert!(parse_config(&provider(json!([1]))).is_err());
}
