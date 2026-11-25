use clap::Parser;
use devcode::agent::ScriptedModel;
use devcode::config::{preset, Config, ProviderConfig, ProviderKind};
use devcode::{Profile, Session};

fn provider(connector: &str, base_url: &str) -> ProviderConfig {
    ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: base_url.into(),
        api_key_env: String::new(),
        connector: Some(connector.into()),
        model: "gemini-3.8-flash".into(),
        context_window: 1234,
        max_rpm: None,
        extra_body: Default::default(),
    }
}

fn session(root: &std::path::Path, provider: Option<ProviderConfig>) -> Session {
    let config = Config {
        provider,
        ..Default::default()
    };
    Session::new(
        root.to_path_buf(),
        config,
        Profile::Ask,
        Box::new(ScriptedModel::new(vec![])),
        devcode::tools::registry(root.to_path_buf(), false),
    )
}

// One test: it sets DEVCODE_CONFIG_DIR, and the environment is process-wide.
#[test]
fn switching_provider_model_and_effort() {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::env::set_var("DEVCODE_CONFIG_DIR", store.path().join("config"));

    // Gemini: effort is accepted, shown in the label, and cleared again.
    let gemini = provider("gemini", preset("gemini").unwrap().base_url);
    let mut s = session(root.path(), Some(gemini));
    let warning = s.set_effort(Some("high")).unwrap();
    assert!(warning.is_some(), "no stored key, so a warning is expected");
    assert_eq!(
        s.provider_label(),
        "gemini · gemini-3.8-flash · effort high"
    );
    assert!(s.set_effort(Some("extreme")).is_err());
    assert_eq!(s.effort_levels(), ["minimal", "low", "medium", "high"]);
    s.set_effort(None).unwrap();
    assert_eq!(s.provider_label(), "gemini · gemini-3.8-flash");

    // A pro model has fewer levels; an effort it rejects is dropped on switch.
    s.set_effort(Some("minimal")).unwrap();
    s.set_model("gemini-3-pro-preview").unwrap();
    assert_eq!(s.provider_label(), "gemini · gemini-3-pro-preview");
    assert_eq!(s.effort_levels(), ["low", "high"]);
    assert!(s.set_effort(Some("medium")).is_err());
    // Unknown model ids offer no effort rather than a guess.
    s.set_model("gemini-x").unwrap();
    assert!(s.effort_levels().is_empty());
    assert!(s.set_effort(Some("low")).is_err());
    s.set_model("gemini-3.8-flash").unwrap();
    assert!(s.set_model("  ").is_err());

    // Provider switch uses the preset's default model and drops provider-specific options.
    s.set_effort(Some("low")).unwrap();
    s.set_provider("nim").unwrap();
    assert_eq!(
        s.provider_label(),
        format!("nim · {}", preset("nim").unwrap().default_model)
    );
    // Effort is not wired for NIM.
    assert!(s.set_effort(Some("high")).is_err());
    assert!(s.set_provider("nope").is_err());

    // No provider yet: switching to a preset creates one; model and effort need one first.
    let mut empty = session(root.path(), None);
    assert!(empty.set_model("m").is_err());
    assert!(empty.set_effort(Some("low")).is_err());
    empty.set_provider("gemini").unwrap();
    assert!(empty.has_provider());

    // CLI --provider applies any preset, not only nim.
    let cli = devcode::Cli::parse_from(["devcode", "--provider", "gemini"]);
    let mut cfg = Config {
        provider: Some(provider("nim", preset("nim").unwrap().base_url)),
        ..Default::default()
    };
    // Keep a custom model across the CLI switch.
    if let Some(p) = &mut cfg.provider {
        p.model = "my-custom-model".into();
    }
    cli.apply_provider_preset(&mut cfg);
    let p = cfg.provider.unwrap();
    assert_eq!(p.connector.as_deref(), Some("gemini"));
    assert_eq!(p.model, "my-custom-model");
    assert_eq!(p.api_key_env, "GEMINI_API_KEY");
}
