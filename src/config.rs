use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

pub fn default_test_globs() -> Vec<String> {
    vec![
        "tests/**".to_string(),
        "**/*_test.*".to_string(),
        "**/*.spec.*".to_string(),
        "**/test_*.*".to_string(),
    ]
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub verification: VerificationConfig,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_env_name")]
    pub api_key_env: String,
    #[serde(default)]
    pub connector: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_rpm: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_compat")]
    OpenaiCompat,
    #[serde(rename = "anthropic")]
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationConfig {
    #[serde(default)]
    pub test_command: Option<String>,
    #[serde(default = "default_test_globs")]
    pub test_globs: Vec<String>,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            test_command: None,
            test_globs: default_test_globs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(
                    f,
                    "failed to read configuration from {}: {}",
                    path.display(),
                    source
                )
            }
            ConfigError::Parse(err) => {
                write!(f, "configuration syntax/validation error: {}", err)
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse(err) => Some(err),
        }
    }
}

pub fn provider_from_preset(name: &str) -> Option<ProviderConfig> {
    let preset = preset(name)?;
    Some(ProviderConfig {
        kind: preset.kind,
        base_url: preset.base_url.into(),
        api_key_env: preset.api_key_env.into(),
        connector: Some(preset.name.into()),
        model: preset.default_model.into(),
        context_window: 32_000,
        max_rpm: None,
        extra_body: Default::default(),
    })
}

/// Merges project config, user-global provider, and the nim preset default.
pub fn resolve_config(root: &Path, cli: &crate::cli::Cli) -> Result<Config, ConfigError> {
    let settings_dir = crate::credentials::config_dir().map_err(settings_io_error)?;
    resolve_config_at(root, cli, &settings_dir)
}

pub fn resolve_config_at(
    root: &Path,
    cli: &crate::cli::Cli,
    settings_dir: &Path,
) -> Result<Config, ConfigError> {
    let mut config = load_config(root)?;
    if config.provider.is_none() {
        let settings = crate::settings::load_user_settings(settings_dir).map_err(|e| match e {
            crate::settings::SettingsError::Io { path, source } => ConfigError::Io { path, source },
            crate::settings::SettingsError::Parse(source) => ConfigError::Parse(source),
        })?;
        if let Some(p) = settings.resolved_provider() {
            config.provider = Some(p);
        }
    }
    if config.provider.is_none() {
        config.provider = provider_from_preset("nim");
    }
    cli.apply_provider_preset(&mut config);
    Ok(config)
}

fn settings_io_error(e: std::io::Error) -> ConfigError {
    ConfigError::Io {
        path: PathBuf::from("settings"),
        source: e,
    }
}

pub fn load_config(root: &Path) -> Result<Config, ConfigError> {
    let config_path = root.join(".devcode.json");
    let contents = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(source) => {
            return Err(ConfigError::Io {
                path: config_path,
                source,
            })
        }
    };

    parse_config(&contents)
}

pub fn parse_config(json_str: &str) -> Result<Config, ConfigError> {
    serde_json::from_str(json_str).map_err(ConfigError::Parse)
}

fn deserialize_env_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let name = String::deserialize(deserializer)?;
    if !name.is_empty()
        && (!name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            || !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
    {
        return Err(serde::de::Error::custom(
            "api_key_env must be an environment variable name",
        ));
    }
    Ok(name)
}

/// Known providers for the UI. Models are not listed here; the picker asks the provider.
pub struct Preset {
    pub name: &'static str,
    pub kind: ProviderKind,
    pub base_url: &'static str,
    pub api_key_env: &'static str,
    /// A model id that worked in a live run when this was written.
    pub default_model: &'static str,
}

pub const PRESETS: [Preset; 5] = [
    Preset {
        name: "nim",
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://integrate.api.nvidia.com/v1",
        api_key_env: "NVIDIA_API_KEY",
        default_model: "nvidia/nemotron-3.5-super-vl-120b-a12b",
    },
    Preset {
        name: "gemini",
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        api_key_env: "GEMINI_API_KEY",
        // Prefer a catalog id that answers under load; gemini-3.8-flash often 503'd in RES-013.
        default_model: "gemini-3-flash-preview",
    },
    Preset {
        name: "openai",
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://api.openai.com/v1",
        api_key_env: "OPENAI_API_KEY",
        default_model: "gpt-4o",
    },
    // Native Anthropic Messages API (P10a). Peer bar: OpenCode/pi anthropic providers.
    Preset {
        name: "anthropic",
        kind: ProviderKind::Anthropic,
        base_url: "https://api.anthropic.com",
        api_key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-sonnet-4-20250514",
    },
    // Other vendors / Claude via OpenRouter's OpenAI-compatible API.
    Preset {
        name: "openrouter",
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://openrouter.ai/api/v1",
        api_key_env: "OPENROUTER_API_KEY",
        default_model: "anthropic/claude-sonnet-4",
    },
];

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

impl ProviderConfig {
    /// A config that points at a known provider's URL but names no connector gets that
    /// provider's connector, so keys stored with /connect or `auth login` are found.
    pub fn infer_connector(&mut self) {
        if self.connector.is_some() {
            return;
        }
        let url = self.base_url.trim_end_matches('/');
        if let Some(p) = PRESETS.iter().find(|p| p.base_url == url) {
            self.connector = Some(p.name.into());
        }
    }

    /// Reasoning-effort values this model accepts; empty when none.
    /// Unknown models get none rather than a guess that could produce a 400.
    pub fn effort_levels(&self) -> Vec<&'static str> {
        if !self.supports_effort() {
            return Vec::new();
        }
        let m = self.model.as_str();
        if self.connector.as_deref() == Some("gemini")
            || self.base_url.contains("generativelanguage.googleapis.com")
            || self.base_url.contains("cloudcode-pa.googleapis.com")
        {
            let flash = m.contains("flash");
            let pro = m.contains("pro");
            if m.starts_with("gemini-3") {
                if pro {
                    vec!["low", "high"]
                } else if flash {
                    vec!["minimal", "low", "medium", "high"]
                } else {
                    Vec::new()
                }
            } else if m.starts_with("gemini-2.5") {
                if pro {
                    vec!["low", "medium", "high"]
                } else if flash {
                    vec!["none", "low", "medium", "high"]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        } else if self.kind == ProviderKind::Anthropic
            || self.connector.as_deref() == Some("anthropic")
        {
            if claude_supports_thinking(m) {
                vec!["low", "medium", "high"]
            } else {
                Vec::new()
            }
        } else if m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4")
            || m.starts_with("gpt-5")
            || m.contains("/o1")
            || m.contains("/o3")
            || m.contains("/o4")
            || m.contains("/gpt-5")
            || claude_supports_thinking(m)
        {
            vec!["low", "medium", "high"]
        } else {
            Vec::new()
        }
    }

    /// True when the backend supports `/effort` reasoning controls.
    pub fn supports_effort(&self) -> bool {
        self.connector.as_deref() == Some("gemini")
            || self.base_url.contains("generativelanguage.googleapis.com")
            || self.base_url.contains("cloudcode-pa.googleapis.com")
            || self.kind == ProviderKind::Anthropic
            || matches!(
                self.connector.as_deref(),
                Some("anthropic" | "openai" | "openrouter")
            )
            || self.base_url.contains("api.openai.com")
            || self.base_url.contains("openrouter.ai")
    }

    /// Apply a named preset's connector, URL, and key env. Keeps a custom model unless
    /// empty or still set to another preset's default (so `--provider gemini` after the
    /// nim fallback does not keep a NIM model id).
    pub fn apply_named_preset(&mut self, name: &str) {
        let Some(p) = preset(name) else {
            return;
        };
        let replace_model = self.model.is_empty()
            || PRESETS
                .iter()
                .any(|other| other.name != p.name && other.default_model == self.model);
        self.kind = p.kind;
        self.connector = Some(p.name.into());
        self.base_url = p.base_url.into();
        self.api_key_env = p.api_key_env.into();
        if replace_model {
            self.model = p.default_model.into();
        }
    }

    pub fn apply_nim_preset(&mut self) {
        self.apply_named_preset("nim");
    }
}

fn deserialize_extra_body<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<serde_json::Map<String, serde_json::Value>, D::Error> {
    let body = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
    if body
        .keys()
        .any(|key| matches!(key.as_str(), "model" | "messages" | "tools" | "stream"))
    {
        return Err(serde::de::Error::custom(
            "extra_body cannot override model, messages, tools, or stream",
        ));
    }
    Ok(body)
}

/// Extended thinking exists from Claude 3.7 onward; older Claude models reject `thinking`.
fn claude_supports_thinking(model: &str) -> bool {
    if !model.contains("claude-") {
        return false;
    }
    let legacy = model.contains("claude-instant")
        || model.contains("claude-2")
        || (model.contains("claude-3") && !model.contains("claude-3-7"));
    !legacy
}

#[cfg(test)]
mod thinking_tests {
    use super::claude_supports_thinking;

    #[test]
    fn only_claude_3_7_and_newer_support_thinking() {
        for model in [
            "claude-3-7-sonnet-latest",
            "claude-sonnet-4-20250514",
            "claude-opus-4-1",
            "anthropic/claude-sonnet-4",
            "claude-haiku-4-5",
        ] {
            assert!(claude_supports_thinking(model), "{model}");
        }
        for model in [
            "claude-3-haiku-20240307",
            "claude-3-5-sonnet-latest",
            "anthropic/claude-3-opus",
            "claude-2.1",
            "claude-instant-1.2",
            "gpt-5",
        ] {
            assert!(!claude_supports_thinking(model), "{model}");
        }
    }
}
