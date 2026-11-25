use clap::Parser;
use devcode::cli::Cli;
use devcode::config::{load_config, provider_from_preset, resolve_config_at, Config, ProviderKind};
use devcode::{load_user_settings, save_user_settings, UserSettings};
use std::fs;

#[test]
fn resolve_config_empty_project_uses_nim_preset() {
    let root = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    // No settings file.
    let cli = Cli::try_parse_from(["devcode"]).unwrap();
    let cfg =
        resolve_config_at(root.path(), &cli, config_dir.path()).expect("resolve should succeed");
    let p = cfg.provider.expect("nim preset");
    assert_eq!(p.connector.as_deref(), Some("nim"));
    assert_eq!(p.kind, ProviderKind::OpenaiCompat);
    assert!(!p.model.is_empty());
}

#[test]
fn resolve_config_user_settings_override_empty_project() {
    let root = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let provider = provider_from_preset("gemini").expect("gemini");
    save_user_settings(
        config_dir.path(),
        &UserSettings {
            provider: Some(provider),
            ..Default::default()
        },
    )
    .unwrap();
    let cli = Cli::try_parse_from(["devcode"]).unwrap();
    let cfg = resolve_config_at(root.path(), &cli, config_dir.path()).unwrap();
    assert_eq!(
        cfg.provider.as_ref().and_then(|p| p.connector.as_deref()),
        Some("gemini")
    );
}

#[test]
fn resolve_config_project_provider_wins_over_user_settings() {
    let root = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    save_user_settings(
        config_dir.path(),
        &UserSettings {
            default_provider: Some("gemini".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let project = serde_json::json!({
        "provider": {
            "kind": "openai_compat",
            "base_url": "https://integrate.api.nvidia.com/v1",
            "connector": "nim",
            "model": "custom/model-from-project"
        }
    });
    fs::write(
        root.path().join(".devcode.json"),
        serde_json::to_string(&project).unwrap(),
    )
    .unwrap();
    let cli = Cli::try_parse_from(["devcode"]).unwrap();
    let cfg = resolve_config_at(root.path(), &cli, config_dir.path()).unwrap();
    assert_eq!(
        cfg.provider.as_ref().map(|p| p.model.as_str()),
        Some("custom/model-from-project")
    );
}

#[test]
fn user_settings_round_trip_denies_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, r#"{"unknown": true}"#).unwrap();
    let err = load_user_settings(dir.path()).unwrap_err();
    assert!(err.to_string().contains("unknown"));
}

#[test]
fn invalid_settings_do_not_silently_select_a_provider() {
    let root = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cli = Cli::try_parse_from(["devcode"]).unwrap();
    fs::write(dir.path().join("settings.json"), "{").unwrap();
    assert!(resolve_config_at(root.path(), &cli, dir.path()).is_err());
    fs::remove_file(dir.path().join("settings.json")).unwrap();
    fs::create_dir(dir.path().join("settings.json")).unwrap();
    assert!(resolve_config_at(root.path(), &cli, dir.path()).is_err());
}

#[test]
fn failed_settings_replace_keeps_destination_and_removes_temp() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::create_dir(&path).unwrap();
    assert!(save_user_settings(dir.path(), &UserSettings::default()).is_err());
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn load_config_still_omits_provider_without_project_file() {
    let root = tempfile::tempdir().unwrap();
    let cfg = load_config(root.path()).unwrap();
    assert_eq!(cfg, Config::default());
    assert!(cfg.provider.is_none());
}
