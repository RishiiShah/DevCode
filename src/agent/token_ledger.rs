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
    pub(crate) fn total(&self) -> u64 {
        self.system_prompt
            .saturating_add(self.tool_schemas)
            .saturating_add(self.history)
            .saturating_add(self.file_reads)
            .saturating_add(self.command_output)
    }

    /// File and search results ingested since the last assistant message (NFR-4 is per turn).
    pub(crate) fn latest_file_reads(request: &Request) -> u64 {
        let mut tool_names = BTreeMap::new();
        let mut latest = 0u64;
        for message in &request.messages {
            if message.role == Role::Assistant {
                latest = 0;
            }
            for call in &message.tool_calls {
                tool_names.insert(call.id.as_str(), call.name.as_str());
            }
            let is_read = message
                .tool_call_id
                .as_deref()
                .and_then(|id| tool_names.get(id))
                .is_some_and(|name| matches!(*name, "read_file" | "glob" | "grep"));
            if message.role == Role::Tool && is_read {
                let bytes = serde_json::to_vec(message).map_or(0, |data| data.len());
                latest += bytes.div_ceil(4) as u64;
            }
        }
        latest
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibrated_input: Option<u64>,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_log_order: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<LedgerEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerEvent {
    ToolDowngrade {
        mode: super::tool_protocol::ToolMode,
    },
    Gate {
        outcome: String,
    },
    Checkpoint {
        session: String,
        step: usize,
    },
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
    let (cached, cache_input) =
        entries.iter().fold((0u128, 0u128), |(read, total), row| {
            match row
                .reported
                .cache_read_tokens
                .zip(row.reported.input_tokens)
            {
                Some((cached, input)) if cached <= input => {
                    (read + u128::from(cached), total + u128::from(input))
                }
                _ => (read, total),
            }
        });
    let hit_rate = if cache_input > 0 {
        format!(
            " · cache hit rate {:.1}%",
            100.0 * cached as f64 / cache_input as f64
        )
    } else {
        " · cache hit rate missing".to_owned()
    };
    let (sampled, divergence) =
        entries.iter().fold((0, 0i128), |(count, sum), entry| {
            match entry.reported.input_tokens.zip(entry.calibrated_input) {
                Some((actual, predicted)) => {
                    (count + 1, sum + i128::from(actual) - i128::from(predicted))
                }
                None => (count, sum),
            }
        });
    let calibration = if sampled > 0 {
        format!(
            " · reported input minus calibrated estimate: {divergence:+} across {sampled} calls"
        )
    } else {
        String::new()
    };
    Some(format!(
        "reported: {} input, {} output, {} uncached, {} cache read, {} cache write, {} reasoning · estimated input: {} system, {} schemas, {} history, {} file reads, {} command output{calibration}{hit_rate}",
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

/// A stable provider identifier for calibration. Custom URLs are hashed so URL credentials
/// or private hosts do not appear in the saved ledger.
pub(crate) fn provider_key(provider: &crate::config::ProviderConfig) -> String {
    use sha2::{Digest, Sha256};
    if let Some(connector) = &provider.connector {
        return connector.clone();
    }
    let endpoint = provider.base_url.trim_end_matches('/');
    let digest = format!("{:x}", Sha256::digest(endpoint.as_bytes()));
    format!("{}:{}", provider.kind.as_str(), &digest[..16])
}

#[derive(Debug, Clone, Default)]
pub struct Calibration {
    estimated: u128,
    reported: u128,
}

impl Calibration {
    pub(crate) fn observe(&mut self, estimated: u64, reported: Option<u64>) {
        if let Some(reported) = reported.filter(|&count| count > 0 && estimated > 0) {
            self.estimated = self.estimated.saturating_add(u128::from(estimated));
            self.reported = self.reported.saturating_add(u128::from(reported));
        }
    }

    pub(crate) fn predict(&self, estimate: u64) -> u64 {
        if self.estimated == 0 {
            return estimate;
        }
        u128::from(estimate)
            .saturating_mul(self.reported)
            .div_ceil(self.estimated)
            .min(u128::from(u64::MAX)) as u64
    }

    pub(crate) fn conservative(&self, estimate: u64) -> u64 {
        self.predict(estimate).max(estimate)
    }
}

#[cfg(test)]
mod tests {
    use super::Calibration;

    #[test]
    fn calibration_uses_reported_samples_and_preserves_the_safety_floor() {
        let mut calibration = Calibration::default();
        assert_eq!(calibration.predict(100), 100);
        calibration.observe(100, None);
        calibration.observe(100, Some(160));
        assert_eq!(calibration.predict(100), 160);
        calibration.observe(100, Some(20));
        assert_eq!(calibration.predict(100), 90);
        assert_eq!(calibration.conservative(100), 100);
    }
}
