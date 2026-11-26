use super::{Request, Role, Usage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputEstimate {
    pub system_prompt: u64,
    pub tool_schemas: u64,
    pub history: u64,
    pub file_reads: u64,
    pub command_output: u64,
}

impl InputEstimate {
    pub(crate) fn for_request(request: &Request) -> Self {
        let estimate = |bytes: usize| bytes.div_ceil(4) as u64;
        let mut result = Self {
            system_prompt: estimate(request.system_prompt.len()),
            tool_schemas: if request.tools.is_empty() {
                0
            } else {
                estimate(serde_json::to_vec(&request.tools).map_or(0, |data| data.len()))
            },
            ..Default::default()
        };
        let mut tool_names = BTreeMap::new();
        for message in &request.messages {
            for call in &message.tool_calls {
                tool_names.insert(call.id.as_str(), call.name.as_str());
            }
            let bytes = estimate(serde_json::to_vec(message).map_or(0, |data| data.len()));
            let name = message
                .tool_call_id
                .as_deref()
                .and_then(|id| tool_names.get(id));
            match (message.role, name.copied()) {
                (Role::Tool, Some("read_file" | "glob" | "grep")) => result.file_reads += bytes,
                (
                    Role::Tool,
                    Some(
                        "run_command" | "start_process" | "process_logs" | "stop_process"
                        | "list_processes",
                    ),
                ) => result.command_output += bytes,
                _ => result.history += bytes,
            }
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Provider-reported counters. Null means unavailable.
    pub reported: Usage,
    /// Request component estimates in tokens, using UTF-8 bytes divided by four.
    pub estimated: InputEstimate,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_log_order: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<LedgerEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerEvent {
    Gate { outcome: String },
    Checkpoint { session: String, step: usize },
}

pub fn summary(entries: &[LedgerEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let total = |field: fn(&Usage) -> Option<u64>| {
        let mut sum = 0u64;
        let mut missing = 0;
        for entry in entries {
            match field(&entry.reported) {
                Some(value) => sum = sum.saturating_add(value),
                None => missing += 1,
            }
        }
        if missing == entries.len() {
            "missing".to_owned()
        } else if missing > 0 {
            format!("{sum} ({missing} missing)")
        } else {
            sum.to_string()
        }
    };
    let estimate = |field: fn(&InputEstimate) -> u64| {
        entries
            .iter()
            .map(|entry| field(&entry.estimated))
            .sum::<u64>()
    };
    Some(format!(
        "reported: {} input, {} output, {} uncached, {} cache read, {} cache write, {} reasoning · estimated input: {} system, {} schemas, {} history, {} file reads, {} command output",
        total(|u| u.input_tokens),
        total(|u| u.output_tokens),
        total(|u| u.uncached_input_tokens),
        total(|u| u.cache_read_tokens),
        total(|u| u.cache_write_tokens),
        total(|u| u.reasoning_tokens),
        estimate(|i| i.system_prompt),
        estimate(|i| i.tool_schemas),
        estimate(|i| i.history),
        estimate(|i| i.file_reads),
        estimate(|i| i.command_output),
    ))
}
