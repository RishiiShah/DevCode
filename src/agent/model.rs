use serde::{Deserialize, Serialize};
use std::fmt;

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
    /// Opaque provider token (Gemini `thought_signature`) that must be echoed back with the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
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
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
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
    Network(String),
    EmptyScript,
    RateLimited,
    Other(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::FormatError(msg) => write!(f, "model format error: {}", msg),
            ModelError::Network(msg) => write!(f, "model network error: {}", msg),
            ModelError::RateLimited => write!(f, "rate limited"),
            ModelError::EmptyScript => write!(f, "scripted model has no remaining responses"),
            ModelError::Other(msg) => write!(f, "model error: {}", msg),
        }
    }
}

impl std::error::Error for ModelError {}

pub trait Model: Send + Sync {
    fn redact_sensitive(&self, text: &str) -> String {
        text.to_string()
    }
    fn complete(&self, req: &Request) -> Result<Response, ModelError>;
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
    /// Model ids the backend offers, for the UI's model picker.
    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        Err(ModelError::Other(
            "this backend cannot list models; type an id instead".into(),
        ))
    }
}
