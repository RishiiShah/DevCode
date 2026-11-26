use devcode::config::parse_config;

#[test]
fn known_base_url_without_connector_uses_the_stored_key_name() {
    let mut config = parse_config(
        r#"{"provider":{"kind":"openai_compat","base_url":"https://integrate.api.nvidia.com/v1/","api_key_env":"NVIDIA_API_KEY","model":"m","context_window":10}}"#,
    )
    .unwrap();
    let provider = config.provider.as_mut().unwrap();
    provider.infer_connector();
    assert_eq!(provider.connector.as_deref(), Some("nim"));
}

#[test]
fn explicit_connector_and_unknown_urls_are_left_alone() {
    let mut config = parse_config(
        r#"{"provider":{"kind":"openai_compat","base_url":"https://example.com/v1","model":"m","context_window":10}}"#,
    )
    .unwrap();
    let provider = config.provider.as_mut().unwrap();
    provider.infer_connector();
    assert_eq!(provider.connector, None);
    provider.base_url = "https://integrate.api.nvidia.com/v1".into();
    provider.connector = Some("mine".into());
    provider.infer_connector();
    assert_eq!(provider.connector.as_deref(), Some("mine"));
}
