//! Multi-turn memory and retry of dropped model connections.
use devcode::agent::model::{Message, Model, ModelError, Request, Response, Role, Usage};
use devcode::agent::{run_agent_loop, LoopLimits, ProgressEvent, ProgressSink, ToolRegistry};
use devcode::Profile;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Replies from a script and records every request it receives.
struct Recorder {
    replies: Mutex<VecDeque<Result<Response, ModelError>>>,
    seen: Mutex<Vec<Request>>,
}

impl Recorder {
    fn new(replies: Vec<Result<Response, ModelError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Model for Recorder {
    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.seen.lock().unwrap().push(req.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(ModelError::EmptyScript))
    }
}

fn text(reply: &str) -> Result<Response, ModelError> {
    Ok(Response {
        text: Some(reply.into()),
        tool_calls: Vec::new(),
        usage: Usage::default(),
    })
}

#[test]
fn second_turn_sees_the_first_turn() {
    let model = Recorder::new(vec![text("The port is 5173."), text("Stopping it.")]);
    let tools = ToolRegistry::new();
    let first = run_agent_loop(
        Profile::Ask,
        &model,
        &tools,
        "run the server",
        &LoopLimits::default(),
    )
    .unwrap();
    let limits = LoopLimits {
        history: first.messages.clone(),
        ..Default::default()
    };
    run_agent_loop(Profile::Ask, &model, &tools, "now close it", &limits).unwrap();
    let seen = model.seen.lock().unwrap();
    let second: Vec<(Role, &str)> = seen[1]
        .messages
        .iter()
        .map(|m: &Message| (m.role, m.content.as_str()))
        .collect();
    assert_eq!(
        second,
        vec![
            (Role::User, "run the server"),
            (Role::Assistant, "The port is 5173."),
            (Role::User, "now close it"),
        ]
    );
}

#[test]
fn dropped_connection_is_retried_and_reported() {
    let model = Recorder::new(vec![
        Err(ModelError::Network(
            "Network Error: Connection reset by peer (os error 54)".into(),
        )),
        text("done"),
    ]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let limits = LoopLimits {
        progress: Some(ProgressSink(Arc::new(move |e| {
            sink.lock().unwrap().push(e)
        }))),
        stream: false,
        ..Default::default()
    };
    let result = run_agent_loop(Profile::Ask, &model, &ToolRegistry::new(), "hi", &limits).unwrap();
    assert_eq!(result.messages.last().unwrap().content, "done");
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        ProgressEvent::Retry {
            attempt: 1,
            max: 3,
            ..
        }
    )));
}

#[test]
fn http_errors_are_not_retried_by_the_loop() {
    let model = Recorder::new(vec![
        Err(ModelError::Network("HTTP 401: bad key".into())),
        text("never reached"),
    ]);
    let err = run_agent_loop(
        Profile::Ask,
        &model,
        &ToolRegistry::new(),
        "hi",
        &LoopLimits::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("HTTP 401"));
    assert_eq!(model.seen.lock().unwrap().len(), 1);
}

#[test]
fn message_sent_mid_run_reaches_the_model_at_the_next_step() {
    use devcode::agent::model::ToolCall;
    let call = ToolCall {
        id: "c1".into(),
        name: "nope".into(),
        arguments: "{}".into(),
        thought_signature: None,
    };
    let model = Recorder::new(vec![
        Ok(Response {
            text: Some("working".into()),
            tool_calls: vec![call],
            usage: Usage::default(),
        }),
        text("ok, using port 3000"),
    ]);
    let inbox: devcode::agent::SteerInbox = Default::default();
    inbox.lock().unwrap().push("use port 3000".into());
    let limits = LoopLimits {
        steer: Some(inbox.clone()),
        ..Default::default()
    };
    run_agent_loop(
        Profile::Build,
        &model,
        &ToolRegistry::new(),
        "start",
        &limits,
    )
    .unwrap();
    let seen = model.seen.lock().unwrap();
    let last = seen[1].messages.last().unwrap();
    assert_eq!(
        (last.role, last.content.as_str()),
        (Role::User, "use port 3000")
    );
    assert!(inbox.lock().unwrap().is_empty());
}

#[test]
fn message_sent_while_the_model_answers_continues_the_run() {
    let model = Recorder::new(vec![text("done"), text("also done")]);
    let inbox: devcode::agent::SteerInbox = Default::default();
    let pending = inbox.clone();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let limits = LoopLimits {
        steer: Some(inbox),
        stream: false,
        progress: Some(ProgressSink(Arc::new(move |e| {
            // The user types while the first answer arrives.
            if matches!(e, ProgressEvent::Thinking { step: 1 }) {
                pending.lock().unwrap().push("and add a test".into());
            }
            sink.lock().unwrap().push(e)
        }))),
        ..Default::default()
    };
    let result = run_agent_loop(Profile::Ask, &model, &ToolRegistry::new(), "go", &limits).unwrap();
    assert_eq!(result.messages.last().unwrap().content, "also done");
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, ProgressEvent::Steered { text } if text == "and add a test")));
}
