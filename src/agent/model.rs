use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    /// Opaque provider token that must be echoed back with the call: a Gemini
    /// `thought_signature`, or Anthropic thinking blocks prefixed with `ANTHROPIC_THINKING_TAG`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Prefix marking `ToolCall::thought_signature` as a JSON array of Anthropic thinking blocks.
pub const ANTHROPIC_THINKING_TAG: &str = "anthropic-thinking:";

impl ToolCall {
    /// The Gemini thought signature, ignoring Anthropic thinking data stored in the same slot
    /// (possible after a mid-session provider switch).
    pub fn gemini_signature(&self) -> Option<&str> {
        self.thought_signature
            .as_deref()
            .filter(|sig| !sig.starts_with(ANTHROPIC_THINKING_TAG))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    /// Providers may send partial cumulative usage across multiple stream events.
    pub(crate) fn merge(&mut self, latest: Self) {
        self.input_tokens = latest.input_tokens.or(self.input_tokens);
        self.output_tokens = latest.output_tokens.or(self.output_tokens);
        self.uncached_input_tokens = latest.uncached_input_tokens.or(self.uncached_input_tokens);
        self.cache_read_tokens = latest.cache_read_tokens.or(self.cache_read_tokens);
        self.cache_write_tokens = latest.cache_write_tokens.or(self.cache_write_tokens);
        self.reasoning_tokens = latest.reasoning_tokens.or(self.reasoning_tokens);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub text: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

#[derive(Debug)]
pub enum ModelError {
    FormatError(String),
    WithUsage {
        error: Box<ModelError>,
        usage: Box<Usage>,
    },
    Network(String),
    EmptyScript,
    RateLimited,
    Cancelled,
    Other(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::WithUsage { error, .. } => error.fmt(f),
            ModelError::FormatError(msg) => write!(f, "model format error: {}", msg),
            ModelError::Network(msg) => write!(f, "model network error: {}", msg),
            ModelError::RateLimited => write!(f, "rate limited"),
            ModelError::Cancelled => write!(f, "request cancelled"),
            ModelError::EmptyScript => write!(f, "scripted model has no remaining responses"),
            ModelError::Other(msg) => write!(f, "model error: {}", msg),
        }
    }
}

impl ModelError {
    pub(crate) fn with_usage(self, usage: Usage) -> Self {
        if usage == Usage::default() {
            self
        } else {
            Self::WithUsage {
                error: Box::new(self),
                usage: Box::new(usage),
            }
        }
    }
}

impl std::error::Error for ModelError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamChunk<'a> {
    Text(&'a str),
    Reasoning(&'a str),
}

type CancelHook = Box<dyn Fn() + Send + Sync>;

struct CancelInner {
    cancelled: AtomicBool,
    sleep_gate: (Mutex<()>, Condvar),
    hooks: Mutex<Vec<Weak<CancelHook>>>,
}

/// Thread-safe cancellation token shared between UI and worker loops.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

impl Default for CancelInner {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            sleep_gate: (Mutex::new(()), Condvar::new()),
            hooks: Mutex::new(Vec::new()),
        }
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        {
            let _guard = self
                .inner
                .sleep_gate
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.inner.sleep_gate.1.notify_all();
        }
        let mut hooks = self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner());
        hooks.retain(|weak| {
            if let Some(cb) = weak.upgrade() {
                cb();
                true
            } else {
                false
            }
        });
    }

    pub fn reset(&self) {
        self.inner.cancelled.store(false, Ordering::SeqCst);
        let mut hooks = self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner());
        hooks.retain(|weak| weak.strong_count() > 0);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Sleeps up to `duration`, returning `true` immediately if cancelled.
    pub fn sleep(&self, duration: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        if duration.is_zero() {
            return self.is_cancelled();
        }
        let deadline = std::time::Instant::now() + duration;
        let mut guard = self
            .inner
            .sleep_gate
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        loop {
            if self.is_cancelled() {
                return true;
            }
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (next, _) = self
                .inner
                .sleep_gate
                .1
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;
        }
    }

    /// Registers a wakeup callback invoked on `cancel()`. If already cancelled, invokes it immediately.
    pub(crate) fn register_hook(&self, hook: CancelHook) -> Arc<CancelHook> {
        let arc = Arc::new(hook);
        {
            let mut hooks = self.inner.hooks.lock().unwrap_or_else(|e| e.into_inner());
            hooks.retain(|w| w.strong_count() > 0);
            hooks.push(Arc::downgrade(&arc));
        }
        if self.is_cancelled() {
            arc();
        }
        arc
    }
}

pub(crate) struct GateState<T> {
    pub cancelled: bool,
    pub closed: bool,
    pub queue: VecDeque<T>,
}

pub(crate) struct GateSender<T> {
    shared: Arc<(Mutex<GateState<T>>, Condvar)>,
}

impl<T> GateSender<T> {
    /// Returns `false` if the receiver has closed or cancelled.
    pub fn send(&self, item: T) -> bool {
        let mut guard = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        if guard.closed || guard.cancelled {
            return false;
        }
        guard.queue.push_back(item);
        self.shared.1.notify_all();
        true
    }

    pub fn is_closed(&self) -> bool {
        let guard = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed || guard.cancelled
    }
}

impl<T> Drop for GateSender<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        self.shared.1.notify_all();
    }
}

pub(crate) struct GateReceiver<T> {
    shared: Arc<(Mutex<GateState<T>>, Condvar)>,
    _hook: Option<Arc<CancelHook>>,
}

impl<T> GateReceiver<T> {
    pub fn recv(&self) -> Result<Option<T>, ModelError> {
        let mut guard = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if guard.cancelled {
                return Err(ModelError::Cancelled);
            }
            if let Some(item) = guard.queue.pop_front() {
                return Ok(Some(item));
            }
            if guard.closed {
                return Ok(None);
            }
            guard = self.shared.1.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl<T> Drop for GateReceiver<T> {
    fn drop(&mut self) {
        let mut guard = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        self.shared.1.notify_all();
    }
}

pub(crate) fn cancel_channel<T: Send + 'static>(
    cancel: Option<&CancelToken>,
) -> (GateSender<T>, GateReceiver<T>) {
    let shared = Arc::new((
        Mutex::new(GateState {
            cancelled: cancel.is_some_and(CancelToken::is_cancelled),
            closed: false,
            queue: VecDeque::new(),
        }),
        Condvar::new(),
    ));
    let hook = cancel.map(|token| {
        let weak = Arc::downgrade(&shared);
        token.register_hook(Box::new(move || {
            if let Some(gate) = weak.upgrade() {
                let mut guard = gate.0.lock().unwrap_or_else(|e| e.into_inner());
                guard.cancelled = true;
                gate.1.notify_all();
            }
        }))
    });
    (
        GateSender {
            shared: Arc::clone(&shared),
        },
        GateReceiver {
            shared,
            _hook: hook,
        },
    )
}

pub trait Model: Send + Sync {
    fn redact_sensitive(&self, text: &str) -> String {
        text.to_string()
    }
    fn complete(&self, req: &Request) -> Result<Response, ModelError>;
    fn complete_cancellable(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
    ) -> Result<Response, ModelError> {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(ModelError::Cancelled);
        }
        self.complete(req)
    }
    /// Incremental completion. `on_delta` receives redacted text fragments whose
    /// concatenation equals `redact_sensitive` of the returned `Response.text`.
    /// The response body stays raw; the agent loop redacts it after the call.
    /// Backends that do not stream ignore `on_delta` and call `complete`.
    fn complete_stream(
        &self,
        req: &Request,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Response, ModelError> {
        let _ = on_delta;
        self.complete(req)
    }
    /// Cancellable incremental completion emitting both text and reasoning chunks.
    fn complete_stream_cancellable(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(ModelError::Cancelled);
        }
        self.complete_stream(req, &mut |delta| on_chunk(StreamChunk::Text(delta)))
    }
    /// Whether the endpoint rejected streaming usage for this model.
    fn stream_usage_rejected(&self) -> bool {
        false
    }
    fn set_stream_usage(&self, _enabled: bool) {}

    /// Applies the resolved explicit-cache capabilities for the active model.
    fn set_prompt_cache(&self, _enabled: bool, _tool_breakpoints: bool) {}
    fn model_catalog(
        &self,
    ) -> Result<Vec<(String, crate::capabilities::ModelCapabilities)>, ModelError> {
        self.list_models()
            .map(|ids| ids.into_iter().map(|id| (id, Default::default())).collect())
    }

    /// Model ids the backend offers, for the UI's model picker.
    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        Err(ModelError::Other(
            "this backend cannot list models; type an id instead".into(),
        ))
    }
}
