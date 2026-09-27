use pretty_assertions::assert_eq;

use super::*;

#[test]
fn defaults_to_openrouter_and_resolves_legacy_model_behavior() {
    let config = DecisionModelsConfig::default();

    assert_eq!(config.provider, DecisionModelsProvider::OpenRouter);
    assert_eq!(
        config.endpoint(),
        "https://openrouter.ai/api/alpha/decisions"
    );
    assert_eq!(config.model(None), "~typesafe/jev-latest");
    assert_eq!(config.model(Some("custom/jev")), "custom/jev");
}

#[test]
fn parses_ollaya_settings_and_redacts_the_optional_key() {
    let config = toml::from_str::<DecisionModelsConfig>(
        r#"
provider = "ollaya"
ollaya_model = "decider:0.8b"
ollaya_base_url = "http://127.0.0.1:1234/"
ollaya_api_key = "secret-local-key"
"#,
    )
    .expect("decision model config");

    assert_eq!(config.provider, DecisionModelsProvider::Ollaya);
    assert_eq!(config.endpoint(), "http://127.0.0.1:1234/v1/systemone");
    assert_eq!(config.model(None), "decider:0.8b");
    assert_eq!(config.api_key(), Some("secret-local-key"));
    assert!(!format!("{config:?}").contains("secret-local-key"));
}

#[test]
fn ollaya_uses_the_easy_local_defaults_when_optional_fields_are_omitted() {
    let config = toml::from_str::<DecisionModelsConfig>("provider = 'ollaya'")
        .expect("decision model config");

    assert_eq!(config.endpoint(), "http://127.0.0.1:11435/v1/systemone");
    assert_eq!(config.model(None), "decider:2b");
    assert_eq!(config.api_key(), None);
}
