use super::model::{Message, Model, ModelError, Request, Response, Role, ToolCall, Usage};
use crate::config::ProviderConfig;
use serde_json::{json, Value};
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
        let key = match key {
            Some(key) => key,
            None if config.connector.is_none() && config.api_key_env.is_empty() => String::new(),
            None => {
                let stored = if let Some(name) = &config.connector {
                    let dir = crate::credentials::config_dir()
                        .map_err(|e| ModelError::Other(e.to_string()))?;
                    let root =
                        std::env::current_dir().map_err(|e| ModelError::Other(e.to_string()))?;
                    crate::credentials::ensure_outside_workspace(&root, &dir)
                        .map_err(|e| ModelError::Other(e.to_string()))?;
                    crate::credentials::load(&dir)
                        .map_err(|e| ModelError::Other(e.to_string()))?
                        .remove(name)
                } else {
                    None
                };
                stored.filter(|key| !key.is_empty()).ok_or_else(|| {
                    ModelError::Other(format!(
                        "run `devcode auth login {}` or set {}",
                        config.connector.as_deref().unwrap_or("NAME"),
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

    fn anthropic_messages(messages: &[Message]) -> Result<Vec<Value>, ModelError> {
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
                    out.push(json!({"role": "assistant", "content": blocks}));
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
                    out.push(json!({"role": "user", "content": blocks}));
                    continue;
                }
                Role::System => {}
            }
            index += 1;
        }
        Ok(out)
    }

    pub fn request_body(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
    ) -> Result<Value, ModelError> {
        let messages = Self::anthropic_messages(&req.messages)?;
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
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": max_tokens.unwrap_or(4096),
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "stream": false,
        });
        if !req.system_prompt.is_empty() {
            body["system"] = json!(req.system_prompt);
        }
        if let Some(object) = body.as_object_mut() {
            object.extend(self.config.extra_body.clone());
        }
        Ok(body)
    }

    fn parse_response(value: &Value, req: &Request) -> Result<Response, ModelError> {
        let blocks = value
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::FormatError("missing message content".into()))?;
        let mut text_parts = Vec::new();
        let mut calls = Vec::new();
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
                _ => {}
            }
        }
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
            usage: Usage {
                input_tokens: value
                    .pointer("/usage/input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                output_tokens: value
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            },
        })
    }

    fn apply_headers(&self, call: ureq::Request) -> ureq::Request {
        let mut call = call
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json");
        if !self.key.is_empty() {
            call = call.set("x-api-key", &self.key);
        }
        call
    }

    fn complete_inner(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
        retries: bool,
    ) -> Result<Response, ModelError> {
        let body = self.request_body(req, max_tokens)?;
        let url = format!("{}/messages", api_root(&self.config.base_url));
        let mut rate_attempts = 0;
        let mut server_attempts = 0;
        loop {
            if let Some(rpm) = self.config.max_rpm {
                let mut last = self
                    .last_request
                    .lock()
                    .map_err(|e| ModelError::Other(e.to_string()))?;
                let interval = Duration::from_secs_f64(60.0 / f64::from(rpm));
                if let Some(previous) = *last {
                    std::thread::sleep(interval.saturating_sub(previous.elapsed()));
                }
                *last = Some(Instant::now());
            }
            let call = self.apply_headers(self.client.post(&url));
            match call.send_json(body.clone()) {
                Ok(response) => {
                    let value: Value = response
                        .into_json()
                        .map_err(|e| ModelError::FormatError(self.redact(&e.to_string())))?;
                    return Self::parse_response(&value, req);
                }
                Err(ureq::Error::Status(code, response)) => {
                    let retry_after = response
                        .header("Retry-After")
                        .and_then(|s| s.parse::<u64>().ok());
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
                    if retries && count > 0 && count <= limit {
                        std::thread::sleep(Duration::from_secs(
                            retry_after.unwrap_or(1u64 << (count - 1)),
                        ));
                        continue;
                    }
                    if code == 429 && retries {
                        return Err(ModelError::RateLimited);
                    }
                    let body = response.into_string().unwrap_or_default();
                    return Err(ModelError::Network(
                        self.redact(&format!("HTTP {code}: {body}")),
                    ));
                }
                Err(err) => return Err(ModelError::Network(self.redact(&err.to_string()))),
            }
        }
    }
}

impl Model for Anthropic {
    fn redact_sensitive(&self, text: &str) -> String {
        self.redact(text)
    }

    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.complete_inner(req, None, true)
    }

    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        let url = format!("{}/models", api_root(&self.config.base_url));
        let call = self.apply_headers(self.client.get(&url));
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
        let mut ids: Vec<String> = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::FormatError("model list has no data array".into()))?
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}
