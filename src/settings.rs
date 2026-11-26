use crate::config::{provider_from_preset, ProviderConfig};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettings {
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Selected credential key (`nim` or `nim:work`). Absent means the plain connector key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_slot: Option<String>,
    /// Items on the TUI status line (`/statusline`). Absent means the default set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statusline: Option<Vec<String>>,
}

impl UserSettings {
    pub fn resolved_provider(&self) -> Option<ProviderConfig> {
        if let Some(p) = &self.provider {
            return Some(p.clone());
        }
        self.default_provider
            .as_deref()
            .and_then(provider_from_preset)
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse(serde_json::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Io { path, source } => {
                write!(
                    f,
                    "failed to read settings from {}: {}",
                    path.display(),
                    source
                )
            }
            SettingsError::Parse(err) => write!(f, "settings syntax/validation error: {}", err),
        }
    }
}

impl std::error::Error for SettingsError {}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

pub fn load_user_settings(dir: &Path) -> Result<UserSettings, SettingsError> {
    let path = settings_path(dir);
    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UserSettings::default()),
        Err(source) => {
            return Err(SettingsError::Io { path, source });
        }
    };
    serde_json::from_str(&contents).map_err(SettingsError::Parse)
}

pub fn save_user_settings(dir: &Path, settings: &UserSettings) -> Result<(), SettingsError> {
    fs::create_dir_all(dir).map_err(|source| SettingsError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = settings_path(dir);
    let text = serde_json::to_string_pretty(settings).map_err(|e| SettingsError::Io {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = dir.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        writeln!(file, "{text}")?;
        file.sync_all()?;
        fs::rename(&temp, &path)?;
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|source| SettingsError::Io { path, source })
}

pub fn persist_provider(dir: &Path, provider: &ProviderConfig) -> Result<(), SettingsError> {
    let mut settings = load_user_settings(dir)?;
    settings.provider = Some(provider.clone());
    settings.default_provider = None;
    save_user_settings(dir, &settings)
}

pub fn set_active_slot(dir: &Path, slot: Option<&str>) -> Result<(), SettingsError> {
    let mut settings = load_user_settings(dir)?;
    settings.active_slot = slot.map(str::to_owned);
    save_user_settings(dir, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider_from_preset;

    #[test]
    fn persist_provider_keeps_active_slot() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), "{}\n").unwrap();
        assert!(load_user_settings(dir.path())
            .unwrap()
            .active_slot
            .is_none());

        let nim = provider_from_preset("nim").unwrap();
        save_user_settings(
            dir.path(),
            &UserSettings {
                provider: Some(nim),
                default_provider: Some("gemini".into()),
                active_slot: Some("nim:work".into()),
                statusline: None,
            },
        )
        .unwrap();
        let gemini = provider_from_preset("gemini").unwrap();
        persist_provider(dir.path(), &gemini).unwrap();
        let loaded = load_user_settings(dir.path()).unwrap();
        assert_eq!(loaded.active_slot.as_deref(), Some("nim:work"));
        assert_eq!(
            loaded.provider.unwrap().connector.as_deref(),
            Some("gemini")
        );
        assert!(loaded.default_provider.is_none());
    }
}
