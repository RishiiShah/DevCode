use crate::agent::model::{ToolCall, ToolSchema};
use crate::tools::mutate::MutationContext;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn execute(&self, arguments: &str) -> Result<String, String>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
    pub(crate) context: Option<Arc<Mutex<MutationContext>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            context: None,
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().to_string(), Box::new(tool));
    }

    pub(crate) fn set_test_workspace_hash(&self, hash: String) -> Result<(), String> {
        if let Some(context) = &self.context {
            context
                .lock()
                .map_err(|e| e.to_string())?
                .log
                .test_workspace_hash = Some(hash);
        }
        Ok(())
    }

    pub(crate) fn approval_wait(&self) -> std::time::Duration {
        self.context
            .as_ref()
            .and_then(|c| c.lock().ok())
            .and_then(|c| c.approval_wait.lock().ok().map(|v| *v))
            .unwrap_or_default()
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .values()
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    pub fn execute(&self, call: &ToolCall) -> Result<String, String> {
        match self.tools.get(&call.name) {
            Some(tool) => tool.execute(&call.arguments),
            None => Err(format!("unknown tool '{}'", call.name)),
        }
    }
}
