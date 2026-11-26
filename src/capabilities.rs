use crate::config::{model_context_window, ProviderConfig, ProviderKind, DEFAULT_CONTEXT_WINDOW};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMechanism {
    None,
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditFormat {
    ExactReplacement,
    Patch,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilities {
    pub native_tools: Option<bool>,
    pub parallel_tools: Option<bool>,
    pub streaming: Option<bool>,
    pub streaming_usage: Option<bool>,
    pub reasoning_control: Option<bool>,
    pub cache: Option<CacheMechanism>,
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    pub vision: Option<bool>,
    pub edit_format: Option<EditFormat>,
    /// Price in USD millionths per million tokens.
    pub input_price: Option<u64>,
    pub output_price: Option<u64>,
    pub cache_read_price: Option<u64>,
    pub cache_write_price: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRecord {
    pub values: ModelCapabilities,
    pub sources: BTreeMap<&'static str, &'static str>,
}

pub(crate) fn resolve(
    provider: &ProviderConfig,
    configured: &ModelCapabilities,
    reported: &ModelCapabilities,
    legacy_window: Option<u64>,
    stream_usage_rejected: bool,
) -> CapabilityRecord {
    let known_window = model_context_window(&provider.model);
    let connector = provider.connector.as_deref();
    let bundled_cache = match (provider.kind, connector, known_window) {
        (ProviderKind::Anthropic, _, Some(_)) => Some(CacheMechanism::Explicit),
        (_, Some("openai" | "gemini"), Some(_)) => Some(CacheMechanism::Automatic),
        _ => None,
    };
    let mut sources = BTreeMap::new();
    macro_rules! choose {
        ($field:ident, $config:expr, $catalog:expr, $bundled:expr, $default:expr) => {{
            let (value, source) = if let Some(value) = $config {
                (Some(value), "config")
            } else if let Some(value) = $catalog {
                (Some(value), "provider")
            } else if let Some(value) = $bundled {
                (Some(value), "model")
            } else {
                ($default, "default")
            };
            sources.insert(stringify!($field), source);
            value
        }};
        ($field:ident, $bundled:expr, $default:expr) => {
            choose!(
                $field,
                configured.$field,
                reported.$field,
                $bundled,
                $default
            )
        };
    }
    let recognized_tools = known_window.map(|_| true);
    let mut values = ModelCapabilities {
        native_tools: choose!(native_tools, recognized_tools, None),
        parallel_tools: choose!(parallel_tools, None, None),
        streaming: choose!(streaming, None, Some(true)),
        streaming_usage: choose!(streaming_usage, None, Some(true)),
        reasoning_control: choose!(
            reasoning_control,
            known_window.map(|_| !provider.effort_levels().is_empty()),
            Some(false)
        ),
        cache: choose!(cache, bundled_cache, Some(CacheMechanism::None)),
        context_window: choose!(
            context_window,
            configured
                .context_window
                .filter(|n| *n > 0)
                .or(legacy_window),
            reported.context_window.filter(|n| *n > 0),
            known_window,
            Some(DEFAULT_CONTEXT_WINDOW)
        ),
        max_output: choose!(max_output, None, None),
        vision: choose!(vision, None, None),
        edit_format: choose!(edit_format, None, Some(EditFormat::ExactReplacement)),
        input_price: choose!(input_price, None, None),
        output_price: choose!(output_price, None, None),
        cache_read_price: choose!(cache_read_price, None, None),
        cache_write_price: choose!(cache_write_price, None, None),
    };
    if stream_usage_rejected {
        values.streaming_usage = Some(false);
        sources.insert("streaming_usage", "runtime downgrade");
    }
    CapabilityRecord { values, sources }
}
