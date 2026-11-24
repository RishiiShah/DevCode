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
