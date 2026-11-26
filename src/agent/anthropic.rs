use super::model::{
    CancelToken, Message, Model, ModelError, Request, Response, Role, StreamChunk, ToolCall, Usage,
    ANTHROPIC_THINKING_TAG,
};
use super::sse::{
    drive_ureq_response, sleep_cancellable, spawn_http_json, HttpEvent, RedactTail, SseParser,
};
use crate::config::ProviderConfig;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Anthropic {
    config: ProviderConfig,
    key: String,
    client: ureq::Agent,
    last_request: Mutex<Option<Instant>>,
}

fn api_root(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    }
}

struct ToolSlot {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct StreamState {
    tail: RedactTail,
    reasoning_tail: RedactTail,
    text: String,
    tools: BTreeMap<u64, ToolSlot>,
    /// Thinking and redacted_thinking blocks by content index, rebuilt from deltas.
    thinking: BTreeMap<u64, Value>,
    usage: Usage,
}

impl StreamState {
    fn new(key: &str) -> Self {
        Self {
            tail: RedactTail::new(key),
            reasoning_tail: RedactTail::new(key),
            text: String::new(),
            tools: BTreeMap::new(),
            thinking: BTreeMap::new(),
            usage: Usage::default(),
        }
    }
}

/// `Ok(true)` means sleep already happened and the caller should resend.
/// `Err(RateLimited)` matches `complete_inner` after the 429 cap.
fn retry_pause(
    code: u16,
    retry_after: Option<u64>,
    rate_attempts: &mut u32,
    server_attempts: &mut u32,
    retries: bool,
    cancel: Option<&CancelToken>,
) -> Result<bool, ModelError> {
    let count = if code == 429 {
        *rate_attempts += 1;
        *rate_attempts
    } else if code >= 500 {
        *server_attempts += 1;
        *server_attempts
    } else {
        0
    };
    let limit = if code == 429 { 5 } else { 2 };
    if retries && count > 0 && count <= limit {
        sleep_cancellable(
            cancel,
            Duration::from_secs(retry_after.unwrap_or(1u64 << (count - 1))),
        )?;
        return Ok(true);
    }
    if code == 429 && retries {
        return Err(ModelError::RateLimited);
    }
    Ok(false)
}

impl Anthropic {
    pub fn new(config: ProviderConfig) -> Result<Self, ModelError> {
        if config.extra_body.keys().any(|key| {
            matches!(
                key.as_str(),
                "model" | "messages" | "tools" | "stream" | "system"
            )
        }) {
            return Err(ModelError::Other(
                "extra_body cannot override model, messages, tools, stream, or system".into(),
            ));
        }
        let key = if config.api_key_env.is_empty() {
            None
        } else {
            std::env::var(&config.api_key_env)
                .ok()
                .filter(|key| !key.is_empty())
        };
        let switched = config
            .connector
            .as_deref()
            .and_then(crate::credentials::switched_key);
        let key = switched.or(key);
        let key = match key {
            Some(key) => key,
            None if config.connector.is_none() && config.api_key_env.is_empty() => String::new(),
            None => {
                let (label, stored) = if let Some(name) = &config.connector {
                    let dir = crate::credentials::config_dir()
                        .map_err(|e| ModelError::Other(e.to_string()))?;
                    let root =
                        std::env::current_dir().map_err(|e| ModelError::Other(e.to_string()))?;
                    crate::credentials::ensure_outside_workspace(&root, &dir)
                        .map_err(|e| ModelError::Other(e.to_string()))?;
                    let stored = crate::credentials::stored_key(&dir, name)
                        .map_err(|e| ModelError::Other(e.to_string()))?;
                    let label = if stored.as_ref().is_some_and(|key| !key.is_empty()) {
                        name.clone()
                    } else {
                        crate::credentials::selected_slot_name(&dir, name)
                            .map_err(|e| ModelError::Other(e.to_string()))?
                    };
                    (label, stored)
                } else {
                    ("NAME".to_owned(), None)
                };
                stored.filter(|key| !key.is_empty()).ok_or_else(|| {
                    ModelError::Other(format!(
                        "run `devcode auth login {label}` or set {}",
                        if config.api_key_env.is_empty() {
                            "API_KEY_ENV"
                        } else {
                            &config.api_key_env
                        }
                    ))
                })?
            }
        };
        if config.max_rpm == Some(0) {
            return Err(ModelError::Other(
                "provider.max_rpm must be positive".into(),
            ));
        }
        Ok(Self {
            config,
            key,
            last_request: Mutex::new(None),
            client: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(120))
                .build(),
        })
    }

    fn redact(&self, text: &str) -> String {
        if self.key.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.key, "[REDACTED]")
        }
    }

    fn pace(&self, cancel: Option<&CancelToken>) -> Result<(), ModelError> {
        if let Some(rpm) = self.config.max_rpm {
            let wait = {
                let mut last = self
                    .last_request
                    .lock()
                    .map_err(|e| ModelError::Other(e.to_string()))?;
                let interval = Duration::from_secs_f64(60.0 / f64::from(rpm));
                let wait = last
                    .map(|previous| interval.saturating_sub(previous.elapsed()))
                    .unwrap_or(Duration::ZERO);
                *last = Some(Instant::now());
                wait
            };
            sleep_cancellable(cancel, wait)?;
        }
        Ok(())
    }

    fn anthropic_messages(
        messages: &[Message],
        include_thinking: bool,
    ) -> Result<Vec<Value>, ModelError> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < messages.len() {
            let message = &messages[index];
            match message.role {
                Role::User => {
                    out.push(json!({
                        "role": "user",
                        "content": message.content,
                    }));
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    // Anthropic requires the turn's thinking blocks, signatures intact, ahead of
                    // `tool_use` when thinking is enabled.
                    if include_thinking {
                        blocks.extend(stored_thinking(&message.tool_calls));
                    }
                    if !message.content.is_empty() {
                        blocks.push(json!({"type": "text", "text": message.content}));
                    }
                    for call in &message.tool_calls {
                        let input: Value = serde_json::from_str(&call.arguments).map_err(|_| {
                            ModelError::FormatError("invalid tool arguments JSON".into())
                        })?;
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": input,
                        }));
                    }
                    if !blocks.is_empty() {
                        out.push(json!({"role": "assistant", "content": blocks}));
                    }
                }
                Role::Tool => {
                    let mut blocks = Vec::new();
                    while index < messages.len() && messages[index].role == Role::Tool {
                        let tool = &messages[index];
                        let id = tool.tool_call_id.as_deref().unwrap_or_default();
                        let mut block = json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": tool.content,
                        });
                        if tool.content.starts_with("Error") {
                            block["is_error"] = json!(true);
                        }
                        blocks.push(block);
                        index += 1;
                    }
                    // A message the user sent mid-run follows the results in the same turn.
                    while index < messages.len() && messages[index].role == Role::User {
                        blocks.push(json!({"type": "text", "text": messages[index].content}));
                        index += 1;
                    }
                    out.push(json!({"role": "user", "content": blocks}));
                    continue;
                }
                Role::System => {}
            }
            index += 1;
        }
        Ok(out)
    }

    fn beta_header(&self, has_thinking: bool) -> Option<String> {
        has_thinking.then(|| "interleaved-thinking-2025-05-14".to_owned())
    }

    pub fn request_body(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
    ) -> Result<Value, ModelError> {
        let thinking_enabled = self
            .config
            .extra_body
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .is_some_and(|level| matches!(level, "low" | "medium" | "high"));
        let messages = Self::anthropic_messages(&req.messages, thinking_enabled)?;
        let tools: Vec<_> = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                })
            })
            .collect();
        let mut extra = self.config.extra_body.clone();
        let effort_budget = extra
            .remove("reasoning_effort")
            .and_then(|v| v.as_str().map(str::to_owned))
            .and_then(|level| match level.as_str() {
                "low" => Some(1024u32),
                "medium" => Some(4096u32),
                "high" => Some(10000u32),
                _ => None,
            });
        let base_max = max_tokens.unwrap_or(4096);
        let effective_max = match effort_budget {
            Some(budget) => base_max.max(budget + 4096),
            None => base_max,
        };
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": effective_max,
            "messages": messages,
            "stream": false,
        });
        // Anthropic rejects `tool_choice` when no tools are declared, and requires an object.
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!({"type": "auto"});
        }
        if let Some(budget) = effort_budget {
            body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
        }
        if !req.system_prompt.is_empty() {
            body["system"] = json!(req.system_prompt);
        }
        if let Some(object) = body.as_object_mut() {
            object.extend(extra);
        }
        Ok(body)
    }

    fn parse_response(value: &Value, req: &Request) -> Result<Response, ModelError> {
        Self::parse_response_inner(value, req).map_err(|error| {
            error.with_usage(anthropic_usage(value.get("usage").unwrap_or(&Value::Null)))
        })
    }

    fn parse_response_inner(value: &Value, req: &Request) -> Result<Response, ModelError> {
        let blocks = value
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::FormatError("missing message content".into()))?;
        let mut text_parts = Vec::new();
        let mut calls = Vec::new();
        let mut thinking = Vec::new();
        for block in blocks {
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match kind {
                "text" => {
                    if let Some(part) = block.get("text").and_then(Value::as_str) {
                        if !part.is_empty() {
                            text_parts.push(part);
                        }
                    }
                }
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ModelError::FormatError("missing tool name".into()))?;
                    if !req.tools.iter().any(|tool| tool.name == name) {
                        return Err(ModelError::FormatError("unknown tool name".into()));
                    }
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ModelError::FormatError("missing tool call id".into()))?;
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    let arguments = serde_json::to_string(&input)
                        .map_err(|e| ModelError::FormatError(e.to_string()))?;
                    calls.push(ToolCall {
                        id: id.into(),
                        name: name.into(),
                        arguments,
                        thought_signature: None,
                    });
                }
                "thinking" | "redacted_thinking" => thinking.push(block.clone()),
                _ => {}
            }
        }
        attach_thinking(&mut calls, thinking)?;
        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };
        if text.is_none() && calls.is_empty() {
            return Err(ModelError::FormatError(
                "response has no text or tool calls; model may lack tool-calling support".into(),
            ));
        }
        Ok(Response {
            text,
            tool_calls: calls,
            usage: anthropic_usage(value.get("usage").unwrap_or(&Value::Null)),
        })
    }

    fn apply_headers(&self, call: ureq::Request, beta: Option<&str>) -> ureq::Request {
        let mut call = call
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json");
        if !self.key.is_empty() {
            call = call.set("x-api-key", &self.key);
        }
        if let Some(beta) = beta {
            call = call.set("anthropic-beta", beta);
        }
        call
    }

    fn complete_inner(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
        retries: bool,
        cancel: Option<&CancelToken>,
    ) -> Result<Response, ModelError> {
        let body = self.request_body(req, max_tokens)?;
        let beta = self.beta_header(body.get("thinking").is_some());
        let url = format!("{}/messages", api_root(&self.config.base_url));
        let mut rate_attempts = 0u32;
        let mut server_attempts = 0u32;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            self.pace(cancel)?;
            let client = self.client.clone();
            let req_url = url.clone();
            let key = self.key.clone();
            let beta_val = beta.clone();
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let mut call = client
                    .post(&req_url)
                    .set("anthropic-version", "2023-06-01")
                    .set("content-type", "application/json");
                if !key.is_empty() {
                    call = call.set("x-api-key", &key);
                }
                if let Some(b) = beta_val.as_deref() {
                    call = call.set("anthropic-beta", b);
                }
                drive_ureq_response(call.send_json(payload), false, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => return Self::parse_response(&value, req),
                Some(HttpEvent::Status {
                    code,
                    retry_after,
                    body: err_body,
                }) => {
                    if retry_pause(
                        code,
                        retry_after,
                        &mut rate_attempts,
                        &mut server_attempts,
                        retries,
                        cancel,
                    )? {
                        continue;
                    }
                    return Err(ModelError::Network(
                        self.redact(&format!("HTTP {code}: {err_body}")),
                    ));
                }
                Some(HttpEvent::Format(err)) => {
                    return Err(ModelError::FormatError(self.redact(&err)));
                }
                Some(HttpEvent::Transport(err)) => {
                    return Err(ModelError::Network(self.redact(&err)));
                }
                Some(HttpEvent::Bytes(_)) | Some(HttpEvent::Eof) | None => {
                    return Err(ModelError::Network("unexpected end of response".into()));
                }
            }
        }
    }

    fn complete_stream_inner(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        let mut body = self.request_body(req, None)?;
        body["stream"] = json!(true);
        let beta = self.beta_header(body.get("thinking").is_some());
        let url = format!("{}/messages", api_root(&self.config.base_url));
        let mut rate_attempts = 0u32;
        let mut server_attempts = 0u32;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            self.pace(cancel)?;
            let client = self.client.clone();
            let req_url = url.clone();
            let key = self.key.clone();
            let beta_val = beta.clone();
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let mut call = client
                    .post(&req_url)
                    .set("anthropic-version", "2023-06-01")
                    .set("content-type", "application/json");
                if !key.is_empty() {
                    call = call.set("x-api-key", &key);
                }
                if let Some(b) = beta_val.as_deref() {
                    call = call.set("anthropic-beta", b);
                }
                drive_ureq_response(call.send_json(payload), true, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => return Self::parse_response(&value, req),
                // Status is known before any success-body byte. One non-streaming attempt.
                Some(HttpEvent::Status { code: 400, .. }) => {
                    return self.complete_cancellable(req, cancel);
                }
                Some(HttpEvent::Status {
                    code,
                    retry_after,
                    body: err_body,
                }) => {
                    if retry_pause(
                        code,
                        retry_after,
                        &mut rate_attempts,
                        &mut server_attempts,
                        true,
                        cancel,
                    )? {
                        continue;
                    }
                    return Err(ModelError::Network(
                        self.redact(&format!("HTTP {code}: {err_body}")),
                    ));
                }
                Some(HttpEvent::Format(err)) => {
                    return Err(ModelError::FormatError(self.redact(&err)));
                }
                Some(HttpEvent::Transport(err)) => {
                    return Err(ModelError::Network(self.redact(&err)));
                }
                Some(first) => {
                    let mut parser = SseParser::new();
                    let mut state = StreamState::new(&self.key);
                    let mut done = false;
                    let mut current = Some(first);
                    while let Some(ev) = current {
                        match ev {
                            HttpEvent::Bytes(chunk) => {
                                let proceed = parser
                                    .push(&chunk, &mut |name, data| {
                                        self.on_sse_event(name, data, &mut state, on_chunk)
                                    })
                                    .map_err(|error| error.with_usage(state.usage.clone()))?;
                                if !proceed {
                                    done = true;
                                    break;
                                }
                            }
                            HttpEvent::Eof => break,
                            HttpEvent::Transport(err) => {
                                return Err(
                                    ModelError::Network(self.redact(&err)).with_usage(state.usage)
                                );
                            }
                            HttpEvent::Format(err) => {
                                return Err(ModelError::FormatError(self.redact(&err))
                                    .with_usage(state.usage));
                            }
                            HttpEvent::Json(_) | HttpEvent::Status { .. } => break,
                        }
                        current = rx
                            .recv()
                            .map_err(|error| error.with_usage(state.usage.clone()))?;
                    }
                    if !done {
                        parser
                            .finish(&mut |name, data| {
                                self.on_sse_event(name, data, &mut state, on_chunk)
                            })
                            .map_err(|error| error.with_usage(state.usage.clone()))?;
                    }
                    state
                        .reasoning_tail
                        .finish(&mut |piece| on_chunk(StreamChunk::Reasoning(piece)));
                    state
                        .tail
                        .finish(&mut |piece| on_chunk(StreamChunk::Text(piece)));
                    let usage = state.usage.clone();
                    return self
                        .assemble_stream(state, req)
                        .map_err(|error| error.with_usage(usage));
                }
                None => return Err(ModelError::Network("unexpected end of response".into())),
            }
        }
    }

    fn on_sse_event(
        &self,
        name: &str,
        data: &str,
        state: &mut StreamState,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<bool, ModelError> {
        if name == "ping" {
            return Ok(true);
        }
        if name == "error" {
            return Err(ModelError::Network(
                self.redact(&format!("stream error: {data}")),
            ));
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|e| ModelError::FormatError(self.redact(&e.to_string())))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .filter(|kind| !kind.is_empty())
            .unwrap_or(name);
        match kind {
            "ping" => Ok(true),
            "error" => Err(ModelError::Network(
                self.redact(&format!("stream error: {data}")),
            )),
            "message_start" => {
                if let Some(usage) = value.pointer("/message/usage") {
                    state.usage.merge(anthropic_usage(usage));
                }
                Ok(true)
            }
            "content_block_start" => {
                note_block_start(&value, state)?;
                Ok(true)
            }
            "content_block_delta" => {
                note_block_delta(&value, state, on_chunk)?;
                Ok(true)
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    state.usage.merge(anthropic_usage(usage));
                }
                Ok(true)
            }
            "message_stop" => Ok(false),
            _ => Ok(true),
        }
    }

    fn assemble_stream(&self, state: StreamState, req: &Request) -> Result<Response, ModelError> {
        let mut calls = Vec::new();
        for (_, slot) in state.tools {
            let name = slot
                .name
                .ok_or_else(|| ModelError::FormatError("missing tool name".into()))?;
            if !req.tools.iter().any(|tool| tool.name == name) {
                return Err(ModelError::FormatError("unknown tool name".into()));
            }
            let id = slot
                .id
                .ok_or_else(|| ModelError::FormatError("missing tool call id".into()))?;
            let arguments = if slot.arguments.is_empty() {
                "{}".to_owned()
            } else {
                slot.arguments
            };
            serde_json::from_str::<Value>(&arguments)
                .map_err(|e| ModelError::FormatError(self.redact(&e.to_string())))?;
            calls.push(ToolCall {
                id,
                name,
                arguments,
                thought_signature: None,
            });
        }
        attach_thinking(&mut calls, state.thinking.into_values().collect())?;
        let text = if state.text.is_empty() {
            None
        } else {
            Some(state.text)
        };
        if text.is_none() && calls.is_empty() {
            return Err(ModelError::FormatError(
                "response has no text or tool calls; model may lack tool-calling support".into(),
            ));
        }
        Ok(Response {
            text,
            tool_calls: calls,
            usage: state.usage,
        })
    }
}

fn anthropic_usage(value: &Value) -> Usage {
    let count = |key| value.get(key).and_then(Value::as_u64);
    let uncached_input_tokens = count("input_tokens");
    let cache_read_tokens = count("cache_read_input_tokens");
    let cache_write_tokens = count("cache_creation_input_tokens");
    Usage {
        input_tokens: uncached_input_tokens.map(|input| {
            input
                .saturating_add(cache_read_tokens.unwrap_or_default())
                .saturating_add(cache_write_tokens.unwrap_or_default())
        }),
        output_tokens: count("output_tokens"),
        uncached_input_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens: value
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64),
    }
}

fn note_block_start(value: &Value, state: &mut StreamState) -> Result<(), ModelError> {
    let Some(block) = value.get("content_block") else {
        return Ok(());
    };
    let kind = block.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("tool_use" | "thinking" | "redacted_thinking")) {
        return Ok(());
    }
    let index = value
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| ModelError::FormatError("missing content block index".into()))?;
    if kind != Some("tool_use") {
        state.thinking.insert(index, block.clone());
        return Ok(());
    }
    state.tools.insert(
        index,
        ToolSlot {
            id: block.get("id").and_then(Value::as_str).map(str::to_owned),
            name: block.get("name").and_then(Value::as_str).map(str::to_owned),
            arguments: String::new(),
        },
    );
    Ok(())
}

fn note_block_delta(
    value: &Value,
    state: &mut StreamState,
    on_chunk: &mut dyn FnMut(StreamChunk<'_>),
) -> Result<(), ModelError> {
    let delta = value.get("delta");
    match delta
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "text_delta" => {
            if let Some(piece) = delta
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str)
            {
                state.text.push_str(piece);
                state
                    .tail
                    .push(piece, &mut |s| on_chunk(StreamChunk::Text(s)));
            }
        }
        "thinking_delta" => {
            if let Some(piece) = delta
                .and_then(|item| item.get("thinking"))
                .and_then(Value::as_str)
            {
                if let Some(block) = value
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|index| state.thinking.get_mut(&index))
                {
                    append_str_field(block, "thinking", piece);
                }
                state
                    .reasoning_tail
                    .push(piece, &mut |s| on_chunk(StreamChunk::Reasoning(s)));
            }
        }
        "signature_delta" => {
            if let (Some(piece), Some(block)) = (
                delta
                    .and_then(|item| item.get("signature"))
                    .and_then(Value::as_str),
                value
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|index| state.thinking.get_mut(&index)),
            ) {
                append_str_field(block, "signature", piece);
            }
        }
        "input_json_delta" => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| ModelError::FormatError("missing content block index".into()))?;
            let slot = state.tools.get_mut(&index).ok_or_else(|| {
                ModelError::FormatError("tool delta for content block that never started".into())
            })?;
            if let Some(partial) = delta
                .and_then(|item| item.get("partial_json"))
                .and_then(Value::as_str)
            {
                slot.arguments.push_str(partial);
            }
        }
        _ => {}
    }
    Ok(())
}

fn append_str_field(block: &mut Value, field: &str, piece: &str) {
    let current = block.get(field).and_then(Value::as_str).unwrap_or_default();
    block[field] = json!(format!("{current}{piece}"));
}

/// Stores a turn's thinking blocks on its first tool call so they can be replayed with it.
/// Turns without tool calls drop them: Anthropic only requires them ahead of `tool_use`.
fn attach_thinking(calls: &mut [ToolCall], thinking: Vec<Value>) -> Result<(), ModelError> {
    if thinking.is_empty() {
        return Ok(());
    }
    if let Some(first) = calls.first_mut() {
        let raw =
            serde_json::to_string(&thinking).map_err(|e| ModelError::FormatError(e.to_string()))?;
        first.thought_signature = Some(format!("{ANTHROPIC_THINKING_TAG}{raw}"));
    }
    Ok(())
}

/// Thinking blocks stored by `attach_thinking` on an assistant turn's tool calls.
fn stored_thinking(calls: &[ToolCall]) -> Vec<Value> {
    calls
        .iter()
        .filter_map(|call| call.thought_signature.as_deref())
        .filter_map(|sig| sig.strip_prefix(ANTHROPIC_THINKING_TAG))
        .filter_map(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .flatten()
        .collect()
}

impl Model for Anthropic {
    fn redact_sensitive(&self, text: &str) -> String {
        self.redact(text)
    }

    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.complete_cancellable(req, None)
    }

    fn complete_cancellable(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
    ) -> Result<Response, ModelError> {
        self.complete_inner(req, None, true, cancel)
    }

    fn complete_stream(
        &self,
        req: &Request,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Response, ModelError> {
        self.complete_stream_cancellable(req, None, &mut |chunk| {
            if let StreamChunk::Text(piece) = chunk {
                on_delta(piece);
            }
        })
    }

    fn complete_stream_cancellable(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        self.complete_stream_inner(req, cancel, on_chunk)
    }

    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        self.model_catalog()
            .map(|models| models.into_iter().map(|(id, _)| id).collect())
    }

    fn model_catalog(
        &self,
    ) -> Result<Vec<(String, crate::capabilities::ModelCapabilities)>, ModelError> {
        let url = format!("{}/models", api_root(&self.config.base_url));
        let beta = self.beta_header(false);
        let call = self.apply_headers(self.client.get(&url), beta.as_deref());
        let value: Value = match call.call() {
            Ok(response) => response
                .into_json()
                .map_err(|e| ModelError::FormatError(self.redact(&e.to_string())))?,
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                return Err(ModelError::Network(
                    self.redact(&format!("HTTP {code}: {body}")),
                ));
            }
            Err(err) => return Err(ModelError::Network(self.redact(&err.to_string()))),
        };
        let models: std::collections::BTreeMap<_, _> = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::FormatError("model list has no data array".into()))?
            .iter()
            .filter_map(|row| {
                let id = row.get("id")?.as_str()?;
                let caps = crate::capabilities::ModelCapabilities {
                    context_window: row.get("max_input_tokens").and_then(Value::as_u64),
                    max_output: row.get("max_tokens").and_then(Value::as_u64),
                    reasoning_control: row
                        .pointer("/capabilities/thinking/supported")
                        .and_then(Value::as_bool),
                    vision: row
                        .pointer("/capabilities/image_input/supported")
                        .and_then(Value::as_bool),
                    ..Default::default()
                };
                Some((id.to_owned(), caps))
            })
            .collect();
        Ok(models.into_iter().collect())
    }
}
