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
    pub model: String,
    pub context_window: u64,
    #[serde(default)]
    pub max_rpm: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_compat")]
    OpenaiCompat,
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

impl ProviderConfig {
    pub fn apply_nim_preset(&mut self) {
        if self.base_url.is_empty() {
            self.base_url = "https://integrate.api.nvidia.com/v1".into();
        }
        if self.api_key_env.is_empty() {
            self.api_key_env = "NVIDIA_API_KEY".into();
        }
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
