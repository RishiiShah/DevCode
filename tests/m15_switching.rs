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

    // Native Anthropic preset switches kind (P10a) and supports effort (P3).
    empty.set_provider("anthropic").unwrap();
    assert_eq!(
        empty.provider_label(),
        format!("anthropic · {}", preset("anthropic").unwrap().default_model)
    );
    assert_eq!(empty.effort_levels(), ["low", "medium", "high"]);
    let _ = empty.set_effort(Some("medium")).unwrap();
    assert!(empty.provider_label().contains("effort medium"));

    // OpenAI reasoning models also support effort (P3).
    empty.set_provider("openai").unwrap();
    empty.set_model("o3").unwrap();
    assert_eq!(empty.effort_levels(), ["low", "medium", "high"]);

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

#[test]
fn per_model_context_window_updates_on_switch_and_preserves_unknown_config() {
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::env::set_var("DEVCODE_CONFIG_DIR", store.path().join("config"));

    let mut cfg = provider("nim", preset("nim").unwrap().base_url);
    cfg.model = "custom-local-model".into();
    cfg.context_window = 16_384;
    let mut s = session(root.path(), Some(cfg));
    assert_eq!(s.context_window(), Some(16_384));
    assert_eq!(s.context_window_source(), Some("config"));

    // Known preset catalog models get their real window, not 32k.
    s.set_model("nvidia/nemotron-3.5-super-vl-120b-a12b")
        .unwrap();
    assert_eq!(s.context_window(), Some(131_072));
    assert_eq!(s.context_window_source(), Some("model"));
    assert!(!devcode::discipline::compaction::should_compact(
        10_000,
        s.context_window().unwrap()
    ));

    // Switching back to an unknown model preserves the configured value.
    s.set_model("another-unknown-model").unwrap();
    assert_eq!(s.context_window(), Some(16_384));
    assert_eq!(s.context_window_source(), Some("config"));

    // Switching providers picks up the preset model's window.
    s.set_provider("gemini").unwrap();
    assert_eq!(s.context_window(), Some(1_048_576));
    assert_eq!(s.context_window_source(), Some("model"));
}

#[test]
fn session_list_models_filters_non_chat_entries() {
    use devcode::agent::{Model, ModelError, Request, Response};

    struct MixedCatalogModel;
    impl Model for MixedCatalogModel {
        fn complete(&self, _: &Request) -> Result<Response, ModelError> {
            unreachable!()
        }
        fn list_models(&self) -> Result<Vec<String>, ModelError> {
            Ok(vec![
                "gpt-4o".into(),
                "text-embedding-3-large".into(),
                "bge-reranker-large".into(),
                "llama-guard-3".into(),
                "nsfw-detector".into(),
                "tts-1".into(),
                "whisper-1".into(),
                "shield-safety".into(),
                "claude-sonnet-4".into(),
            ])
        }
    }

    let root = tempfile::tempdir().unwrap();
    let s = Session::new(
        root.path().to_path_buf(),
        Config::default(),
        Profile::Ask,
        Box::new(MixedCatalogModel),
        devcode::tools::registry(root.path().to_path_buf(), false),
    );
    assert_eq!(s.list_models().unwrap(), vec!["gpt-4o", "claude-sonnet-4"]);
}

#[test]
fn model_capabilities_override_the_bundled_window_for_one_model() {
    let cfg: Config = serde_json::from_value(serde_json::json!({
        "provider": {"kind": "openai_compat", "connector": "openai", "model": "gpt-5"},
        "model_capabilities": {
            "openai": {"gpt-5": {"context_window": 123_456, "streaming": false}}
        }
    }))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new(
        dir.path().to_path_buf(),
        cfg,
        Profile::Ask,
        Box::new(ScriptedModel::new(vec![])),
        devcode::tools::registry(dir.path().to_path_buf(), false),
    );
    assert_eq!(s.context_window(), Some(123_456));
    assert_eq!(s.context_window_source(), Some("config"));
    s.set_model("gpt-4o").unwrap();
    assert_eq!(s.context_window(), Some(128_000));
    s.set_model("gpt-5").unwrap();
    assert_eq!(s.context_window(), Some(123_456));
}
