use super::model::{Model, ModelError, Request, Response, ToolCall, Usage};
use crate::config::ProviderConfig;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct OpenAiCompat {
    config: ProviderConfig,
    key: String,
    client: ureq::Agent,
    last_request: Mutex<Option<Instant>>,
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
}

impl OpenAiCompat {
    pub fn complete_once(
        &self,
        req: &Request,
        max_tokens: u32,
    ) -> Result<(Response, Option<String>), ModelError> {
        self.complete_inner(req, Some(max_tokens), false)
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
                if let Some(sig) = &c.thought_signature { call["extra_content"] = json!({"google":{"thought_signature":sig}}); }
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

    fn complete_inner(
        &self,
        req: &Request,
        max_tokens: Option<u32>,
        retries: bool,
    ) -> Result<(Response, Option<String>), ModelError> {
        let body = self.request_body(req, max_tokens);
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
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
            let mut call = self.client.post(&url);
            if !self.key.is_empty() {
                call = call.set("Authorization", &format!("Bearer {}", self.key));
            }
            match call.send_json(body.clone()) {
                Ok(response) => {
                    let value: Value = response
                        .into_json()
                        .map_err(|e| ModelError::FormatError(self.redact(&e.to_string())))?;
                    let message = value
                        .pointer("/choices/0/message")
                        .ok_or_else(|| ModelError::FormatError("missing message".into()))?;
                    let text = message
                        .get("content")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let mut calls = Vec::new();
                    if let Some(items) = message.get("tool_calls").and_then(Value::as_array) {
                        for item in items {
                            let name = item
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    ModelError::FormatError("missing tool name".into())
                                })?;
                            if !req.tools.iter().any(|t| t.name == name) {
                                return Err(ModelError::FormatError("unknown tool name".into()));
                            }
                            let arguments = item
                                .pointer("/function/arguments")
                                .and_then(Value::as_str)
                                .ok_or_else(|| {
                                    ModelError::FormatError("missing tool arguments".into())
                                })?;
                            serde_json::from_str::<Value>(arguments).map_err(|_| {
                                ModelError::FormatError("invalid tool arguments JSON".into())
                            })?;
                            let id = item.get("id").and_then(Value::as_str).ok_or_else(|| {
                                ModelError::FormatError("missing tool call id".into())
                            })?;
                            calls.push(ToolCall {
                                id: id.into(),
                                name: name.into(),
                                arguments: arguments.into(),
                                thought_signature: item
                                    .pointer("/extra_content/google/thought_signature")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            });
                        }
                    }
                    if text.is_none() && calls.is_empty() {
                        return Err(ModelError::FormatError("response has no text or tool calls; model may lack tool-calling support".into()));
                    }
                    let finish_reason = value
                        .pointer("/choices/0/finish_reason")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    return Ok((
                        Response {
                            text,
                            tool_calls: calls,
                            usage: Usage {
                                input_tokens: value
                                    .pointer("/usage/prompt_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_default(),
                                output_tokens: value
                                    .pointer("/usage/completion_tokens")
                                    .and_then(Value::as_u64)
                                    .unwrap_or_default(),
                            },
                        },
                        finish_reason,
                    ));
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

impl Model for OpenAiCompat {
    fn redact_sensitive(&self, text: &str) -> String {
        self.redact(text)
    }
    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.complete_inner(req, None, true)
            .map(|(response, _)| response)
    }
    fn list_models(&self) -> Result<Vec<String>, ModelError> {
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
        let mut ids: Vec<String> = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::FormatError("model list has no data array".into()))?
            .iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            // Gemini's compatibility endpoint prefixes ids with "models/".
            .map(|id| id.strip_prefix("models/").unwrap_or(id).to_owned())
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}
