use super::model::{
    CancelToken, Model, ModelError, Request, Response, StreamChunk, ToolCall, Usage,
};
use super::sse::{
    drive_ureq_response, sleep_cancellable, spawn_http_json, HttpEvent, RedactTail, SseParser,
};
use crate::config::ProviderConfig;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct OpenAiCompat {
    config: ProviderConfig,
    key: String,
    client: ureq::Agent,
    last_request: Mutex<Option<Instant>>,
    stream_usage_rejected: AtomicBool,
    stream_usage_enabled: AtomicBool,
}

impl OpenAiCompat {
    pub fn new(config: ProviderConfig) -> Result<Self, ModelError> {
        if config
            .extra_body
            .keys()
            .any(|key| matches!(key.as_str(), "model" | "messages" | "tools" | "stream"))
        {
            return Err(ModelError::Other(
                "extra_body cannot override model, messages, tools, or stream".into(),
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
            stream_usage_rejected: AtomicBool::new(false),
            stream_usage_enabled: AtomicBool::new(true),
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
}

impl OpenAiCompat {
    pub fn complete_once(
        &self,
        req: &Request,
        max_tokens: u32,
    ) -> Result<(Response, Option<String>), ModelError> {
        self.complete_inner(req, Some(max_tokens), false, None)
    }

    pub fn request_body(&self, req: &Request, max_tokens: Option<u32>) -> Value {
        let mut messages = Vec::new();
        if !req.system_prompt.is_empty() {
            messages.push(json!({"role":"system","content":req.system_prompt}));
        }
        messages.extend(req.messages.iter().map(|m| {
            let mut v = json!({"role":m.role,"content":m.content});
            if let Some(id) = &m.tool_call_id { v["tool_call_id"] = json!(id); }
            if !m.tool_calls.is_empty() { v["tool_calls"] = json!(m.tool_calls.iter().map(|c| {
                let mut call = json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}});
                if let Some(sig) = c.gemini_signature() { call["extra_content"] = json!({"google":{"thought_signature":sig}}); }
                call
            }).collect::<Vec<_>>()); }
            v
        }));
        let tools: Vec<_> = req.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect();
        let mut body = json!({"model":self.config.model,"messages":messages,"tools":tools,"tool_choice":"auto","stream":false});
        if let Some(object) = body.as_object_mut() {
            object.extend(self.config.extra_body.clone());
        }
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        body
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

    /// `Ok(delay)` retries the same request. Rate-limit exhaustion and every
    /// other status are errors. Limits match `complete`: retry while the 429
    /// count is <= 5 and the 5xx count is <= 2.
    fn handle_status_parts(
        &self,
        code: u16,
        retry_after: Option<u64>,
        body: String,
        retries: bool,
        rate_attempts: &mut u32,
        server_attempts: &mut u32,
    ) -> Result<Duration, ModelError> {
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
            return Ok(Duration::from_secs(
                retry_after.unwrap_or(1u64 << (count - 1)),
            ));
        }
        if code == 429 && retries {
            return Err(ModelError::RateLimited);
        }
        Err(ModelError::Network(
            self.redact(&format!("HTTP {code}: {body}")),
        ))
    }

    fn complete_inner(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
        retries: bool,
        cancel: Option<&CancelToken>,
    ) -> Result<(Response, Option<String>), ModelError> {
        let body = self.request_body(req, max_tokens);
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut rate_attempts = 0;
        let mut server_attempts = 0;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            self.pace(cancel)?;
            let client = self.client.clone();
            let req_url = url.clone();
            let key = self.key.clone();
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let mut call = client.post(&req_url);
                if !key.is_empty() {
                    call = call.set("Authorization", &format!("Bearer {key}"));
                }
                drive_ureq_response(call.send_json(payload), false, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => return parse_chat_completion(&value, req),
                Some(HttpEvent::Status {
                    code,
                    retry_after,
                    body,
                }) => {
                    let delay = self.handle_status_parts(
                        code,
                        retry_after,
                        body,
                        retries,
                        &mut rate_attempts,
                        &mut server_attempts,
                    )?;
                    sleep_cancellable(cancel, delay)?;
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

    fn on_sse_data(
        &self,
        data: &str,
        accum: &mut StreamAccum,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<bool, ModelError> {
        if data.trim() == "[DONE]" {
            return Ok(false);
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|err| ModelError::FormatError(self.redact(&err.to_string())))?;
        if let Some(reasoning) = value
            .pointer("/choices/0/delta/reasoning_content")
            .and_then(Value::as_str)
            .or_else(|| {
                value
                    .pointer("/choices/0/delta/reasoning")
                    .and_then(Value::as_str)
            })
        {
            accum.reasoning_redact.push(reasoning, &mut |piece| {
                on_chunk(StreamChunk::Reasoning(piece))
            });
        }
        if let Some(content) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            accum.raw.push_str(content);
            accum
                .redact
                .push(content, &mut |piece| on_chunk(StreamChunk::Text(piece)));
        }
        if let Some(items) = value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
        {
            for item in items {
                apply_tool_call(item, &mut accum.calls)?;
            }
        }
        if let Some(prompt) = value
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
        {
            accum.usage.input_tokens = prompt;
        }
        if let Some(completion) = value
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
        {
            accum.usage.output_tokens = completion;
        }
        Ok(true)
    }

    fn stream_via_channel(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        let mut body = self.request_body(req, None);
        body["stream"] = json!(true);
        if self.stream_usage_enabled.load(Ordering::Relaxed)
            && !self.stream_usage_rejected.load(Ordering::Relaxed)
        {
            body["stream_options"] = json!({"include_usage": true});
        } else if let Some(object) = body.as_object_mut() {
            object.remove("stream_options");
        }
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut rate_attempts = 0;
        let mut server_attempts = 0;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            self.pace(cancel)?;
            let client = self.client.clone();
            let req_url = url.clone();
            let key = self.key.clone();
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let mut call = client.post(&req_url);
                if !key.is_empty() {
                    call = call.set("Authorization", &format!("Bearer {key}"));
                }
                drive_ureq_response(call.send_json(payload), true, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => {
                    return parse_chat_completion(&value, req).map(|(response, _)| response);
                }
                Some(HttpEvent::Status {
                    code: 400,
                    body: error,
                    ..
                }) if body.get("stream_options").is_some()
                    && serde_json::from_str::<Value>(&error).is_ok_and(|value| {
                        matches!(
                            value.pointer("/error/param").and_then(Value::as_str),
                            Some("stream_options" | "stream_options.include_usage")
                        )
                    }) =>
                {
                    self.stream_usage_rejected.store(true, Ordering::Relaxed);
                    body.as_object_mut().unwrap().remove("stream_options");
                }
                Some(HttpEvent::Status { code: 400, .. }) => {
                    return self.complete_cancellable(req, cancel);
                }
                Some(HttpEvent::Status {
                    code,
                    retry_after,
                    body,
                }) => {
                    let delay = self.handle_status_parts(
                        code,
                        retry_after,
                        body,
                        true,
                        &mut rate_attempts,
                        &mut server_attempts,
                    )?;
                    sleep_cancellable(cancel, delay)?;
                }
                Some(HttpEvent::Format(err)) => {
                    return Err(ModelError::FormatError(self.redact(&err)));
                }
                Some(HttpEvent::Transport(err)) => {
                    return Err(ModelError::Network(self.redact(&err)));
                }
                Some(first) => {
                    let mut parser = SseParser::new();
                    let mut accum = StreamAccum {
                        raw: String::new(),
                        calls: BTreeMap::new(),
                        usage: Usage::default(),
                        redact: RedactTail::new(&self.key),
                        reasoning_redact: RedactTail::new(&self.key),
                    };
                    let mut done = false;
                    let mut current = Some(first);
                    while let Some(ev) = current {
                        match ev {
                            HttpEvent::Bytes(chunk) => {
                                let keep_going = parser.push(&chunk, &mut |_event, data| {
                                    self.on_sse_data(data, &mut accum, on_chunk)
                                })?;
                                if !keep_going {
                                    done = true;
                                    break;
                                }
                            }
                            HttpEvent::Eof => break,
                            HttpEvent::Transport(err) => {
                                return Err(ModelError::Network(self.redact(&err)));
                            }
                            HttpEvent::Format(err) => {
                                return Err(ModelError::FormatError(self.redact(&err)));
                            }
                            HttpEvent::Json(_) | HttpEvent::Status { .. } => break,
                        }
                        current = rx.recv()?;
                    }
                    if !done {
                        parser.finish(&mut |_event, data| {
                            self.on_sse_data(data, &mut accum, on_chunk)
                        })?;
                    }
                    accum
                        .reasoning_redact
                        .finish(&mut |piece| on_chunk(StreamChunk::Reasoning(piece)));
                    accum
                        .redact
                        .finish(&mut |piece| on_chunk(StreamChunk::Text(piece)));
                    return assemble_stream(req, accum.raw, accum.calls, accum.usage);
                }
                None => return Err(ModelError::Network("unexpected end of response".into())),
            }
        }
    }
}

impl Model for OpenAiCompat {
    fn stream_usage_rejected(&self) -> bool {
        self.stream_usage_rejected.load(Ordering::Relaxed)
    }

    fn set_stream_usage(&self, enabled: bool) {
        self.stream_usage_enabled.store(enabled, Ordering::Relaxed);
    }

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
            .map(|(response, _)| response)
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
        self.stream_via_channel(req, cancel, on_chunk)
    }
    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        self.model_catalog()
            .map(|models| models.into_iter().map(|(id, _)| id).collect())
    }

    fn model_catalog(
        &self,
    ) -> Result<Vec<(String, crate::capabilities::ModelCapabilities)>, ModelError> {
        let url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        let mut call = self.client.get(&url);
        if !self.key.is_empty() {
            call = call.set("Authorization", &format!("Bearer {}", self.key));
        }
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
                let id = id.strip_prefix("models/").unwrap_or(id);
                let parameters = row.get("supported_parameters").and_then(Value::as_array);
                let supports = |name| {
                    parameters.map(|items| items.iter().any(|item| item.as_str() == Some(name)))
                };
                let caps = crate::capabilities::ModelCapabilities {
                    context_window: row.get("context_length").and_then(Value::as_u64),
                    max_output: row
                        .pointer("/top_provider/max_completion_tokens")
                        .and_then(Value::as_u64),
                    native_tools: supports("tools"),
                    parallel_tools: supports("parallel_tool_calls"),
                    reasoning_control: parameters.map(|items| {
                        items.iter().any(|item| {
                            matches!(item.as_str(), Some("reasoning" | "reasoning_effort"))
                        })
                    }),
                    vision: row
                        .pointer("/architecture/input_modalities")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().any(|item| item.as_str() == Some("image"))),
                    ..Default::default()
                };
                Some((id.to_owned(), caps))
            })
            .collect();
        Ok(models.into_iter().collect())
    }
}

#[derive(Default)]
struct PartialTool {
    id: String,
    name: Option<String>,
    arguments: Option<String>,
    thought_signature: Option<String>,
}

struct StreamAccum {
    raw: String,
    calls: BTreeMap<u64, PartialTool>,
    usage: Usage,
    redact: RedactTail,
    reasoning_redact: RedactTail,
}

fn parse_chat_completion(
    value: &Value,
    req: &Request,
) -> Result<(Response, Option<String>), ModelError> {
    let message = value
        .pointer("/choices/0/message")
        .ok_or_else(|| ModelError::FormatError("missing message".into()))?;
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let mut calls = Vec::new();
    if let Some(items) = message.get("tool_calls").and_then(Value::as_array) {
        for item in items {
            calls.push(validated_tool(
                req,
                item.get("id").and_then(Value::as_str),
                item.pointer("/function/name").and_then(Value::as_str),
                item.pointer("/function/arguments").and_then(Value::as_str),
                item.pointer("/extra_content/google/thought_signature")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )?);
        }
    }
    let finish_reason = value
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let usage = Usage {
        input_tokens: value
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: value
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    };
    finish_response(text, calls, usage, finish_reason)
}

fn validated_tool(
    req: &Request,
    id: Option<&str>,
    name: Option<&str>,
    arguments: Option<&str>,
    thought_signature: Option<String>,
) -> Result<ToolCall, ModelError> {
    let name = name.ok_or_else(|| ModelError::FormatError("missing tool name".into()))?;
    if !req.tools.iter().any(|tool| tool.name == name) {
        return Err(ModelError::FormatError("unknown tool name".into()));
    }
    let arguments =
        arguments.ok_or_else(|| ModelError::FormatError("missing tool arguments".into()))?;
    serde_json::from_str::<Value>(arguments)
        .map_err(|_| ModelError::FormatError("invalid tool arguments JSON".into()))?;
    let id = id.ok_or_else(|| ModelError::FormatError("missing tool call id".into()))?;
    Ok(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
        thought_signature,
    })
}

fn finish_response(
    text: Option<String>,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    finish_reason: Option<String>,
) -> Result<(Response, Option<String>), ModelError> {
    if text.is_none() && tool_calls.is_empty() {
        return Err(ModelError::FormatError(
            "response has no text or tool calls; model may lack tool-calling support".into(),
        ));
    }
    Ok((
        Response {
            text,
            tool_calls,
            usage,
        },
        finish_reason,
    ))
}

fn assemble_stream(
    req: &Request,
    raw_text: String,
    calls: BTreeMap<u64, PartialTool>,
    usage: Usage,
) -> Result<Response, ModelError> {
    let text = (!raw_text.is_empty()).then_some(raw_text);
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (_, call) in calls {
        let id = (!call.id.is_empty()).then_some(call.id);
        // A streamed no-argument call can finish with "" or no arguments chunk at all.
        let arguments = match call.arguments.as_deref() {
            Some(a) if !a.trim().is_empty() => a,
            _ => "{}",
        };
        tool_calls.push(validated_tool(
            req,
            id.as_deref(),
            call.name.as_deref(),
            Some(arguments),
            call.thought_signature,
        )?);
    }
    finish_response(text, tool_calls, usage, None).map(|(response, _)| response)
}

fn apply_tool_call(item: &Value, calls: &mut BTreeMap<u64, PartialTool>) -> Result<(), ModelError> {
    let index = item
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| ModelError::FormatError("missing tool call index".into()))?;
    let call = calls.entry(index).or_default();
    if let Some(id) = item.get("id").and_then(Value::as_str) {
        if call.id.is_empty() && !id.is_empty() {
            call.id = id.to_owned();
        }
    }
    if let Some(name) = item.pointer("/function/name").and_then(Value::as_str) {
        append_piece(&mut call.name, name);
    }
    if let Some(arguments) = item.pointer("/function/arguments").and_then(Value::as_str) {
        append_piece(&mut call.arguments, arguments);
    }
    if let Some(sig) = item
        .pointer("/extra_content/google/thought_signature")
        .and_then(Value::as_str)
    {
        call.thought_signature = Some(sig.to_owned());
    }
    Ok(())
}

fn append_piece(slot: &mut Option<String>, piece: &str) {
    match slot {
        Some(existing) => existing.push_str(piece),
        None => *slot = Some(piece.to_owned()),
    }
}
