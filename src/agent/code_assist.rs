use super::model::{
    CancelToken, Model, ModelError, Request, Response, Role, StreamChunk, ToolCall, Usage,
};
use super::sse::{
    drive_ureq_response, sleep_cancellable, spawn_http_json, HttpEvent, RedactTail, SseParser,
};
use crate::auth::{self, StoredOAuth, DEFAULT_TOKEN_URL};
use crate::config::ProviderConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CODE_ASSIST_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const CODE_ASSIST_VERSION: &str = "v1internal";
static PROMPT_COUNTER: AtomicU64 = AtomicU64::new(1);

const OAUTH_GEMINI_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-3-flash-preview",
    "gemini-3-pro-preview",
];

pub struct CodeAssist {
    config: ProviderConfig,
    config_dir: PathBuf,
    token_url: String,
    endpoint: String,
    client: ureq::Agent,
    last_request: Mutex<Option<Instant>>,
    auth_state: Mutex<StoredOAuth>,
    /// `loadCodeAssist` reported a tier without a project; skip the lookup for this session.
    no_project: AtomicBool,
}

impl CodeAssist {
    pub fn new(config: ProviderConfig) -> Result<Self, ModelError> {
        let dir = crate::credentials::config_dir().map_err(|e| ModelError::Other(e.to_string()))?;
        let root = std::env::current_dir().map_err(|e| ModelError::Other(e.to_string()))?;
        crate::credentials::ensure_outside_workspace(&root, &dir)
            .map_err(|e| ModelError::Other(e.to_string()))?;
        Self::new_at(config, dir, DEFAULT_TOKEN_URL.to_string())
    }

    pub fn new_at(
        config: ProviderConfig,
        config_dir: PathBuf,
        token_url: String,
    ) -> Result<Self, ModelError> {
        if config.max_rpm == Some(0) {
            return Err(ModelError::Other(
                "provider.max_rpm must be positive".into(),
            ));
        }
        let stored = auth::valid_gemini_oauth(&config_dir, &token_url)
            .map_err(ModelError::Other)?
            .ok_or_else(|| {
                ModelError::Other(
                    "run `devcode auth login gemini --oauth` or set GEMINI_API_KEY".into(),
                )
            })?;
        let endpoint = if config.base_url.trim().is_empty()
            || config
                .base_url
                .contains("generativelanguage.googleapis.com")
        {
            CODE_ASSIST_ENDPOINT.to_string()
        } else {
            config.base_url.trim_end_matches('/').to_string()
        };
        Ok(Self {
            config,
            config_dir,
            token_url,
            endpoint,
            client: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(120))
                .build(),
            last_request: Mutex::new(None),
            auth_state: Mutex::new(stored),
            no_project: AtomicBool::new(false),
        })
    }

    fn redact_token(token: &str, text: &str) -> String {
        if token.is_empty() {
            text.to_owned()
        } else {
            text.replace(token, "[REDACTED]")
        }
    }

    fn current_token(&self) -> Result<(String, Option<String>), ModelError> {
        let current = self
            .auth_state
            .lock()
            .map_err(|e| ModelError::Other(e.to_string()))?
            .clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let valid = current
            .expires_at
            .is_none_or(|exp| now.saturating_add(60) < exp);
        if valid {
            return Ok((current.access_token, current.project_id));
        }
        let updated = auth::force_refresh_gemini_oauth(&self.config_dir, &self.token_url, &current)
            .map_err(ModelError::Other)?;
        let out = (updated.access_token.clone(), updated.project_id.clone());
        *self
            .auth_state
            .lock()
            .map_err(|e| ModelError::Other(e.to_string()))? = updated;
        Ok(out)
    }

    fn refresh_on_unauthorized(&self) -> Result<String, ModelError> {
        let current = self
            .auth_state
            .lock()
            .map_err(|e| ModelError::Other(e.to_string()))?
            .clone();
        let updated = auth::force_refresh_gemini_oauth(&self.config_dir, &self.token_url, &current)
            .map_err(ModelError::Other)?;
        let token = updated.access_token.clone();
        *self
            .auth_state
            .lock()
            .map_err(|e| ModelError::Other(e.to_string()))? = updated;
        Ok(token)
    }

    fn method_url(&self, method: &str) -> String {
        format!("{}/{}:{}", self.endpoint, CODE_ASSIST_VERSION, method)
    }

    fn operation_url(&self, name: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint,
            CODE_ASSIST_VERSION,
            name.trim_start_matches('/')
        )
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

    fn ensure_project_id(
        &self,
        token: &mut String,
        cached: Option<String>,
        cancel: Option<&CancelToken>,
    ) -> Result<Option<String>, ModelError> {
        if let Some(id) = cached.filter(|s| !s.is_empty()) {
            return Ok(Some(id));
        }
        if self.no_project.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if let Ok(env_id) = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GOOGLE_CLOUD_PROJECT_ID"))
        {
            if !env_id.trim().is_empty() {
                return Ok(Some(env_id));
            }
        }
        let metadata = json!({
            "ideType": "IDE_UNSPECIFIED",
            "platform": "PLATFORM_UNSPECIFIED",
            "pluginType": "GEMINI",
        });
        let load_res = self.control(
            &self.method_url("loadCodeAssist"),
            token,
            Some(json!({ "metadata": metadata })),
            cancel,
        )?;
        if let Some(project) = load_res
            .get("cloudaicompanionProject")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.cache_project_id(project)?;
            return Ok(Some(project.to_owned()));
        }
        if load_res.get("currentTier").is_some() {
            self.no_project.store(true, Ordering::Relaxed);
            return Ok(None);
        }
        let tier_id = load_res
            .get("allowedTiers")
            .and_then(Value::as_array)
            .and_then(|tiers| {
                tiers
                    .iter()
                    .find(|t| t.get("isDefault").and_then(Value::as_bool) == Some(true))
                    .or_else(|| tiers.first())
            })
            .and_then(|t| t.get("id").and_then(Value::as_str))
            .unwrap_or("free-tier");

        let mut lro = self.control(
            &self.method_url("onboardUser"),
            token,
            Some(json!({
                "tierId": tier_id,
                "metadata": metadata,
            })),
            cancel,
        )?;
        for _ in 0..12 {
            if lro.get("done").and_then(Value::as_bool) == Some(true) {
                break;
            }
            let Some(op_name) = lro.get("name").and_then(Value::as_str) else {
                break;
            };
            sleep_cancellable(cancel, Duration::from_secs(2))?;
            let op_url = self.operation_url(op_name);
            lro = self.control(&op_url, token, None, cancel)?;
        }
        if let Some(project) = lro
            .pointer("/response/cloudaicompanionProject/id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.cache_project_id(project)?;
            return Ok(Some(project.to_owned()));
        }
        Ok(None)
    }

    fn cache_project_id(&self, project: &str) -> Result<(), ModelError> {
        {
            let mut guard = self
                .auth_state
                .lock()
                .map_err(|e| ModelError::Other(e.to_string()))?;
            guard.project_id = Some(project.to_owned());
        }
        auth::update_gemini_project_id(&self.config_dir, project).map_err(ModelError::Other)
    }

    /// Sends a control-plane request: `body` is the POST payload, `None` sends a GET.
    /// A 401 triggers one token refresh and retry; `token` is updated in place so later
    /// requests in the same turn use the fresh token.
    fn control(
        &self,
        url: &str,
        token: &mut String,
        body: Option<Value>,
        cancel: Option<&CancelToken>,
    ) -> Result<Value, ModelError> {
        let mut refreshed = false;
        loop {
            let client = self.client.clone();
            let req_url = url.to_owned();
            let auth_hdr = format!("Bearer {token}");
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let result = match payload {
                    Some(payload) => client
                        .post(&req_url)
                        .set("Authorization", &auth_hdr)
                        .set("Content-Type", "application/json")
                        .send_json(payload),
                    None => client
                        .get(&req_url)
                        .set("Authorization", &auth_hdr)
                        .set("Content-Type", "application/json")
                        .call(),
                };
                drive_ureq_response(result, false, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => return Ok(value),
                Some(HttpEvent::Status { code: 401, .. }) if !refreshed => {
                    *token = self.refresh_on_unauthorized()?;
                    refreshed = true;
                }
                Some(HttpEvent::Status { code, body, .. }) => {
                    return Err(ModelError::Network(Self::redact_token(
                        token,
                        &format!("HTTP {code}: {body}"),
                    )))
                }
                Some(HttpEvent::Format(err)) => {
                    return Err(ModelError::FormatError(Self::redact_token(token, &err)))
                }
                Some(HttpEvent::Transport(err)) => {
                    return Err(ModelError::Network(Self::redact_token(token, &err)))
                }
                _ => return Err(ModelError::Network("unexpected end of response".into())),
            }
        }
    }

    pub fn request_body(&self, req: &Request, project_id: Option<&str>) -> Value {
        // Filled while walking history in order, so a tool result resolves to the most recent
        // call with its id. Fallback ids (`ca_{index}`) repeat across turns.
        let mut tool_names: HashMap<String, String> = HashMap::new();
        let mut contents = Vec::new();
        let mut i = 0;
        while i < req.messages.len() {
            let msg = &req.messages[i];
            match msg.role {
                Role::User => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": msg.content}],
                    }));
                }
                Role::Assistant => {
                    let mut parts = Vec::new();
                    if !msg.content.is_empty() {
                        parts.push(json!({"text": msg.content}));
                    }
                    for call in &msg.tool_calls {
                        tool_names.insert(call.id.clone(), call.name.clone());
                        let args: Value =
                            serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                        let mut part = json!({
                            "functionCall": {
                                "name": call.name,
                                "args": args,
                            }
                        });
                        if let Some(sig) = call.gemini_signature() {
                            part["thoughtSignature"] = json!(sig);
                        }
                        parts.push(part);
                    }
                    if !parts.is_empty() {
                        contents.push(json!({"role": "model", "parts": parts}));
                    }
                }
                Role::Tool => {
                    let mut parts = Vec::new();
                    while i < req.messages.len() && req.messages[i].role == Role::Tool {
                        let tool_msg = &req.messages[i];
                        let fn_name = tool_msg
                            .tool_call_id
                            .as_deref()
                            .and_then(|id| tool_names.get(id).map(String::as_str))
                            .unwrap_or("tool");
                        parts.push(json!({
                            "functionResponse": {
                                "name": fn_name,
                                "response": { "result": tool_msg.content }
                            }
                        }));
                        i += 1;
                    }
                    contents.push(json!({"role": "user", "parts": parts}));
                    continue;
                }
                Role::System => {}
            }
            i += 1;
        }
        let mut inner = json!({ "contents": contents });
        if !req.system_prompt.is_empty() {
            inner["systemInstruction"] = json!({
                "role": "user",
                "parts": [{"text": req.system_prompt}],
            });
        }
        if !req.tools.is_empty() {
            let decls: Vec<_> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect();
            inner["tools"] = json!([{ "functionDeclarations": decls }]);
            inner["toolConfig"] = json!({ "functionCallingConfig": { "mode": "AUTO" } });
        }
        if let Some(effort) = self
            .config
            .extra_body
            .get("reasoning_effort")
            .and_then(Value::as_str)
        {
            if let Some(thinking) = gemini_thinking_config(&self.config.model, effort) {
                inner["generationConfig"] = json!({ "thinkingConfig": thinking });
            }
        }
        let mut envelope = json!({
            "model": self.config.model,
            "user_prompt_id": format!("devcode-{}", PROMPT_COUNTER.fetch_add(1, Ordering::Relaxed)),
            "request": inner,
        });
        if let Some(project) = project_id {
            envelope["project"] = json!(project);
        }
        envelope
    }

    fn complete_inner(
        &self,
        req: &Request,
        stream: bool,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        let (mut token, cached_project) = self.current_token()?;
        let project_id = self.ensure_project_id(&mut token, cached_project, cancel)?;
        let body = self.request_body(req, project_id.as_deref());
        let url = if stream {
            format!("{}?alt=sse", self.method_url("streamGenerateContent"))
        } else {
            self.method_url("generateContent")
        };
        let mut rate_attempts = 0u32;
        let mut server_attempts = 0u32;
        let mut refreshed_401 = false;
        loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            self.pace(cancel)?;
            let client = self.client.clone();
            let req_url = url.clone();
            let auth_hdr = format!("Bearer {token}");
            let payload = body.clone();
            let rx = spawn_http_json(cancel, move |tx| {
                let call = client
                    .post(&req_url)
                    .set("Authorization", &auth_hdr)
                    .set("Content-Type", "application/json");
                drive_ureq_response(call.send_json(payload), stream, tx);
            });
            match rx.recv()? {
                Some(HttpEvent::Json(value)) => return parse_ca_response(&value, req),
                Some(HttpEvent::Status {
                    code: 401,
                    body: err_body,
                    ..
                }) => {
                    if !refreshed_401 {
                        if let Ok(new_token) = self.refresh_on_unauthorized() {
                            token = new_token;
                            refreshed_401 = true;
                            continue;
                        }
                    }
                    return Err(ModelError::Network(Self::redact_token(
                        &token,
                        &format!("HTTP 401: {err_body}"),
                    )));
                }
                Some(HttpEvent::Status {
                    code,
                    retry_after,
                    body: err_body,
                }) => {
                    let count = if code == 429 {
                        rate_attempts += 1;
                        rate_attempts
                    } else if code >= 500 {
                        server_attempts += 1;
                        server_attempts
                    } else {
                        0
                    };
                    let limit = if code == 429 { 5 } else { 2 };
                    if count > 0 && count <= limit {
                        let delay = Duration::from_secs(retry_after.unwrap_or(1u64 << (count - 1)));
                        sleep_cancellable(cancel, delay)?;
                        continue;
                    }
                    if code == 429 {
                        return Err(ModelError::RateLimited);
                    }
                    return Err(ModelError::Network(Self::redact_token(
                        &token,
                        &format!("HTTP {code}: {err_body}"),
                    )));
                }
                Some(HttpEvent::Format(err)) => {
                    return Err(ModelError::FormatError(Self::redact_token(&token, &err)));
                }
                Some(HttpEvent::Transport(err)) => {
                    return Err(ModelError::Network(Self::redact_token(&token, &err)));
                }
                Some(first) => {
                    let mut parser = SseParser::new();
                    let mut state = CaStreamState::new(&token);
                    let mut current = Some(first);
                    let mut done = false;
                    while let Some(ev) = current {
                        match ev {
                            HttpEvent::Bytes(chunk) => {
                                let proceed = parser.push(&chunk, &mut |_name, data| {
                                    apply_ca_event(data, req, &mut state, on_chunk)
                                })?;
                                if !proceed {
                                    done = true;
                                    break;
                                }
                            }
                            HttpEvent::Eof => break,
                            HttpEvent::Transport(err) => {
                                return Err(ModelError::Network(Self::redact_token(&token, &err)));
                            }
                            HttpEvent::Format(err) => {
                                return Err(ModelError::FormatError(Self::redact_token(
                                    &token, &err,
                                )));
                            }
                            HttpEvent::Json(_) | HttpEvent::Status { .. } => break,
                        }
                        current = rx.recv()?;
                    }
                    if !done {
                        parser.finish(&mut |_name, data| {
                            apply_ca_event(data, req, &mut state, on_chunk)
                        })?;
                    }
                    state
                        .reasoning_tail
                        .finish(&mut |p| on_chunk(StreamChunk::Reasoning(p)));
                    state
                        .text_tail
                        .finish(&mut |p| on_chunk(StreamChunk::Text(p)));
                    return finish_ca_stream(state);
                }
                None => return Err(ModelError::Network("unexpected end of response".into())),
            }
        }
    }
}

fn gemini_thinking_config(model: &str, effort: &str) -> Option<Value> {
    if model.starts_with("gemini-3") {
        let level = match effort {
            "minimal" | "low" | "medium" | "high" => effort.to_ascii_uppercase(),
            _ => return None,
        };
        Some(json!({ "thinkingLevel": level, "includeThoughts": true }))
    } else if model.starts_with("gemini-2.5") {
        let budget = match effort {
            "none" => return Some(json!({ "thinkingBudget": 0 })),
            "low" => 1024,
            "medium" => 8192,
            "high" => 24576,
            _ => return None,
        };
        Some(json!({ "thinkingBudget": budget, "includeThoughts": true }))
    } else {
        None
    }
}

struct CaStreamState {
    text: String,
    calls: Vec<ToolCall>,
    usage: Usage,
    text_tail: RedactTail,
    reasoning_tail: RedactTail,
}

impl CaStreamState {
    fn new(token: &str) -> Self {
        Self {
            text: String::new(),
            calls: Vec::new(),
            usage: Usage::default(),
            text_tail: RedactTail::new(token),
            reasoning_tail: RedactTail::new(token),
        }
    }
}

fn apply_ca_event(
    data: &str,
    req: &Request,
    state: &mut CaStreamState,
    on_chunk: &mut dyn FnMut(StreamChunk<'_>),
) -> Result<bool, ModelError> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(trimmed != "[DONE]");
    }
    let value: Value =
        serde_json::from_str(trimmed).map_err(|e| ModelError::FormatError(e.to_string()))?;
    let resp = value.get("response").unwrap_or(&value);
    if let Some(parts) = resp
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    state
                        .reasoning_tail
                        .push(text, &mut |p| on_chunk(StreamChunk::Reasoning(p)));
                } else if !text.is_empty() {
                    state.text.push_str(text);
                    state
                        .text_tail
                        .push(text, &mut |p| on_chunk(StreamChunk::Text(p)));
                }
            }
            if let Some(fc) = part.get("functionCall") {
                state
                    .calls
                    .push(parse_function_call(part, fc, req, state.calls.len())?);
            }
        }
    }
    if let Some(prompt) = resp
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(Value::as_u64)
    {
        state.usage.input_tokens = prompt;
    }
    if let Some(cand) = resp
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(Value::as_u64)
    {
        state.usage.output_tokens = cand;
    }
    Ok(true)
}

fn finish_ca_stream(state: CaStreamState) -> Result<Response, ModelError> {
    let text = (!state.text.is_empty()).then_some(state.text);
    if text.is_none() && state.calls.is_empty() {
        return Err(ModelError::FormatError(
            "response has no text or tool calls; model may lack tool-calling support".into(),
        ));
    }
    Ok(Response {
        text,
        tool_calls: state.calls,
        usage: state.usage,
    })
}

fn parse_ca_response(value: &Value, req: &Request) -> Result<Response, ModelError> {
    let resp = value.get("response").unwrap_or(value);
    let parts = resp
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(|| ModelError::FormatError("missing candidate parts".into()))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for part in parts {
        if part.get("thought").and_then(Value::as_bool) != Some(true) {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                text.push_str(t);
            }
        }
        if let Some(fc) = part.get("functionCall") {
            calls.push(parse_function_call(part, fc, req, calls.len())?);
        }
    }
    let text = (!text.is_empty()).then_some(text);
    if text.is_none() && calls.is_empty() {
        return Err(ModelError::FormatError(
            "response has no text or tool calls; model may lack tool-calling support".into(),
        ));
    }
    Ok(Response {
        text,
        tool_calls: calls,
        usage: Usage {
            input_tokens: resp
                .pointer("/usageMetadata/promptTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            output_tokens: resp
                .pointer("/usageMetadata/candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        },
    })
}

fn parse_function_call(
    part: &Value,
    fc: &Value,
    req: &Request,
    index: usize,
) -> Result<ToolCall, ModelError> {
    let name = fc
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ModelError::FormatError("missing tool name".into()))?;
    if !req.tools.iter().any(|t| t.name == name) {
        return Err(ModelError::FormatError("unknown tool name".into()));
    }
    let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
    let arguments =
        serde_json::to_string(&args).map_err(|e| ModelError::FormatError(e.to_string()))?;
    let sig = part
        .get("thoughtSignature")
        .or_else(|| part.get("thought_signature"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = fc
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("ca_{index}"));
    Ok(ToolCall {
        id,
        name: name.to_owned(),
        arguments,
        thought_signature: sig,
    })
}

impl Model for CodeAssist {
    fn redact_sensitive(&self, text: &str) -> String {
        let token = self
            .auth_state
            .lock()
            .map(|s| s.access_token.clone())
            .unwrap_or_default();
        Self::redact_token(&token, text)
    }

    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.complete_cancellable(req, None)
    }

    fn complete_cancellable(
        &self,
        req: &Request,
        cancel: Option<&CancelToken>,
    ) -> Result<Response, ModelError> {
        self.complete_inner(req, false, cancel, &mut |_| {})
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
        self.complete_inner(req, true, cancel, on_chunk)
    }

    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        Ok(OAUTH_GEMINI_MODELS
            .iter()
            .map(|s| (*s).to_owned())
            .collect())
    }
}
