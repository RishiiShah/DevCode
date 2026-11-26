use devcode::agent::session::RateLimitedModel;
use devcode::agent::{
    run_agent_loop, ExitReason, LoopLimits, Model, ModelError, ProgressEvent, ProgressSink,
    Request, Response, Role, ToolRegistry, Usage,
};
use devcode::Profile;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct SplitModel {
    completes: Arc<Mutex<usize>>,
    streams: Arc<Mutex<usize>>,
}

impl SplitModel {
    fn new() -> (Self, Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
        let completes = Arc::new(Mutex::new(0));
        let streams = Arc::new(Mutex::new(0));
        (
            Self {
                completes: Arc::clone(&completes),
                streams: Arc::clone(&streams),
            },
            completes,
            streams,
        )
    }
}

impl Model for SplitModel {
    fn complete(&self, _: &Request) -> Result<Response, ModelError> {
        *self.completes.lock().unwrap() += 1;
        Ok(hello())
    }

    fn complete_stream(
        &self,
        _: &Request,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Response, ModelError> {
        *self.streams.lock().unwrap() += 1;
        on_delta("hel");
        on_delta("lo");
        Ok(hello())
    }
}

fn hello() -> Response {
    Response {
        text: Some("hello".into()),
        tool_calls: Vec::new(),
        usage: Usage::default(),
    }
}

fn run(
    model: &dyn Model,
    stream: bool,
    progress: Option<ProgressSink>,
) -> devcode::agent::LoopResult {
    let limits = LoopLimits {
        stream,
        progress,
        ..LoopLimits::default()
    };
    run_agent_loop(Profile::Ask, model, &ToolRegistry::new(), "hi", &limits).unwrap()
}

fn assistant_texts(events: &[ProgressEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            ProgressEvent::Assistant { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_final_hello(result: &devcode::agent::LoopResult) {
    assert_eq!(result.exit_reason, ExitReason::Completed);
    let message = result.messages.last().unwrap();
    assert_eq!(message.role, Role::Assistant);
    assert_eq!(message.content, "hello");
}

#[test]
fn sink_and_stream_emits_prefixes_without_complete() {
    let (model, completes, streams) = SplitModel::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&events);
    let result = run(
        &model,
        true,
        Some(ProgressSink(Arc::new(move |event| {
            record.lock().unwrap().push(event);
        }))),
    );
    assert_final_hello(&result);
    assert_eq!(*completes.lock().unwrap(), 0);
    assert_eq!(*streams.lock().unwrap(), 1);
    // The loop also emits the finished assistant text; the TUI drops that repeat.
    assert_eq!(
        assistant_texts(&events.lock().unwrap()),
        vec!["hel", "hello", "hello"]
    );
}

#[test]
fn no_sink_or_stream_off_uses_complete_only() {
    let (model, completes, streams) = SplitModel::new();
    let result = run(&model, true, None);
    assert_final_hello(&result);
    assert_eq!(*completes.lock().unwrap(), 1);
    assert_eq!(*streams.lock().unwrap(), 0);

    let (model, completes, streams) = SplitModel::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&events);
    let result = run(
        &model,
        false,
        Some(ProgressSink(Arc::new(move |event| {
            record.lock().unwrap().push(event);
        }))),
    );
    assert_final_hello(&result);
    assert_eq!(*completes.lock().unwrap(), 1);
    assert_eq!(*streams.lock().unwrap(), 0);
    assert_eq!(assistant_texts(&events.lock().unwrap()), vec!["hello"]);
}

#[test]
fn rate_limited_model_with_sink_calls_inner_stream_once() {
    let (inner, completes, streams) = SplitModel::new();
    let model = RateLimitedModel::new(Box::new(inner), Duration::ZERO);
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&events);
    let result = run(
        &model,
        true,
        Some(ProgressSink(Arc::new(move |event| {
            record.lock().unwrap().push(event);
        }))),
    );
    assert_final_hello(&result);
    assert_eq!(*completes.lock().unwrap(), 0);
    assert_eq!(*streams.lock().unwrap(), 1);
    assert_eq!(
        assistant_texts(&events.lock().unwrap()),
        vec!["hel", "hello", "hello"]
    );
}
