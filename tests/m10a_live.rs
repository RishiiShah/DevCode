use devcode::agent::model::{Message, Request, ToolSchema};
use devcode::agent::openai_compat::OpenAiCompat;
use devcode::config::{ProviderConfig, ProviderKind};

#[test]
#[ignore = "live; spends NIM credits"]
fn nim_smoke() {
    if std::env::var_os("NVIDIA_API_KEY").is_none_or(|v| v.is_empty()) {
        println!("Skipped: NVIDIA_API_KEY is unset or empty");
        return;
    }
    let config = ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://integrate.api.nvidia.com/v1".into(),
        api_key_env: "NVIDIA_API_KEY".into(),
        connector: None,
        model: "nvidia/nemotron-3.5-super-vl-120b-a12b".into(),
        context_window: 131072,
        max_rpm: None,
        extra_body: Default::default(),
    };
    let request = Request {
        system_prompt: String::new(),
        messages: vec![Message::user("What is 17 + 25? Use the add tool.")],
        tools: vec![ToolSchema {
            name: "add".into(),
            description: "Add two numbers".into(),
            parameters: serde_json::json!({"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}),
        }],
    };
    let (response, finish_reason) = OpenAiCompat::new(config)
        .unwrap()
        .complete_once(&request, 512)
        .unwrap();
    println!("tool_calls: {}", !response.tool_calls.is_empty());
    let key = std::env::var("NVIDIA_API_KEY").unwrap();
    for call in &response.tool_calls {
        println!("tool name: {}", call.name.replace(&key, "[REDACTED]"));
        println!(
            "arguments: {}",
            serde_json::from_str::<serde_json::Value>(&call.arguments)
                .unwrap()
                .to_string()
                .replace(&key, "[REDACTED]")
        );
    }
    println!(
        "content length: {}",
        response.text.as_ref().map_or(0, |s| s.len())
    );
    println!(
        "finish_reason: {}",
        finish_reason
            .unwrap_or_default()
            .replace(&key, "[REDACTED]")
    );
    println!(
        "usage: input={} output={}",
        response.usage.input_tokens, response.usage.output_tokens
    );
    assert!(response.usage.input_tokens > 0 && response.usage.output_tokens > 0);
}

#[test]
#[ignore = "live; spends Gemini quota"]
fn gemini_smoke() {
    use devcode::agent::model::Model;
    if std::env::var_os("GEMINI_API_KEY").is_none_or(|v| v.is_empty()) {
        println!("Skipped: GEMINI_API_KEY is unset or empty");
        return;
    }
    let mut config = ProviderConfig {
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai".into(),
        api_key_env: "GEMINI_API_KEY".into(),
        connector: Some("gemini".into()),
        model: devcode::config::preset("gemini")
            .unwrap()
            .default_model
            .into(),
        context_window: 1_048_576,
        max_rpm: None,
        extra_body: Default::default(),
    };
    let client = OpenAiCompat::new(config.clone()).unwrap();
    let models = client
        .list_models()
        .expect("list_models with GEMINI_API_KEY");
    assert!(!models.is_empty(), "Gemini returned an empty model list");
    println!("models listed: {}", models.len());
    let mut candidates = Vec::new();
    if models.iter().any(|id| id == &config.model) {
        candidates.push(config.model.clone());
    }
    for id in &models {
        if id.contains("flash") && !candidates.iter().any(|c| c == id) {
            candidates.push(id.clone());
        }
    }
    assert!(
        !candidates.is_empty(),
        "no flash model in Gemini catalog to smoke"
    );
    let request = Request {
        system_prompt: String::new(),
        messages: vec![Message::user("What is 17 + 25? Use the add tool.")],
        tools: vec![ToolSchema {
            name: "add".into(),
            description: "Add two numbers".into(),
            parameters: serde_json::json!({"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}),
        }],
    };
    let key = std::env::var("GEMINI_API_KEY").unwrap();
    let mut last_err = None;
    for model in candidates {
        println!("trying model: {model}");
        config.model = model;
        let client = OpenAiCompat::new(config.clone()).unwrap();
        // Retries on 503; complete_once does not.
        match client.complete(&request) {
            Ok(response) => {
                println!("tool_calls: {}", !response.tool_calls.is_empty());
                for call in &response.tool_calls {
                    println!("tool name: {}", call.name.replace(&key, "[REDACTED]"));
                    println!(
                        "arguments: {}",
                        serde_json::from_str::<serde_json::Value>(&call.arguments)
                            .unwrap()
                            .to_string()
                            .replace(&key, "[REDACTED]")
                    );
                }
                println!(
                    "content length: {}",
                    response.text.as_ref().map_or(0, |s| s.len())
                );
                println!(
                    "usage: input={} output={}",
                    response.usage.input_tokens, response.usage.output_tokens
                );
                assert!(response.usage.input_tokens > 0 && response.usage.output_tokens > 0);
                return;
            }
            Err(err) => {
                println!(
                    "model failed: {}",
                    client.redact_sensitive(&err.to_string())
                );
                last_err = Some(err);
            }
        }
    }
    panic!(
        "all candidate Gemini models failed: {}",
        last_err
            .map(|e| client.redact_sensitive(&e.to_string()))
            .unwrap_or_default()
    );
}
