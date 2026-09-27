//! Configuration for the decision model used by optional Codex decision features.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::types::RedactedString;

/// Provider used by skill suggestions and TUI autorouting.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DecisionModelsProvider {
    /// Use the hosted OpenRouter Decisions API.
    #[default]
    OpenRouter,
    /// Use a local or user-hosted Ollaya instance.
    Ollaya,
}

/// Shared configuration for skill suggestions and TUI autorouting decisions.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
#[derive(Default)]
pub struct DecisionModelsConfig {
    /// Decision service to use. Defaults to OpenRouter to preserve existing behavior.
    #[serde(default)]
    pub provider: DecisionModelsProvider,

    /// OpenRouter API key. Takes precedence over the legacy `tui.jev_openrouter_api_key`.
    #[serde(default)]
    pub openrouter_api_key: Option<RedactedString>,

    /// Ollaya model to call. Defaults to `decider:2b` when Ollaya is selected.
    #[serde(default)]
    pub ollaya_model: Option<String>,

    /// Root URL of the Ollaya server. Defaults to `http://127.0.0.1:11435`.
    #[serde(default)]
    pub ollaya_base_url: Option<String>,

    /// Optional bearer key for an Ollaya server configured with `OLLAYA_API_KEY`.
    #[serde(default)]
    pub ollaya_api_key: Option<RedactedString>,
}

impl DecisionModelsConfig {
    pub const OPENROUTER_ENDPOINT: &'static str = "https://openrouter.ai/api/alpha/decisions";
    pub const DEFAULT_OLLAYA_MODEL: &'static str = "decider:2b";
    pub const DEFAULT_OLLAYA_BASE_URL: &'static str = "http://127.0.0.1:11435";

    pub fn endpoint(&self) -> String {
        match self.provider {
            DecisionModelsProvider::OpenRouter => Self::OPENROUTER_ENDPOINT.to_string(),
            DecisionModelsProvider::Ollaya => format!(
                "{}/v1/systemone",
                self.ollaya_base_url
                    .as_deref()
                    .filter(|url| !url.trim().is_empty())
                    .unwrap_or(Self::DEFAULT_OLLAYA_BASE_URL)
                    .trim_end_matches('/')
            ),
        }
    }

    pub fn model(&self, openrouter_model: Option<&str>) -> String {
        match self.provider {
            DecisionModelsProvider::OpenRouter => openrouter_model
                .filter(|model| !model.trim().is_empty())
                .unwrap_or("~typesafe/jev-latest")
                .to_string(),
            DecisionModelsProvider::Ollaya => self
                .ollaya_model
                .as_deref()
                .filter(|model| !model.trim().is_empty())
                .unwrap_or(Self::DEFAULT_OLLAYA_MODEL)
                .to_string(),
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        let key = match self.provider {
            DecisionModelsProvider::OpenRouter => self.openrouter_api_key.as_ref(),
            DecisionModelsProvider::Ollaya => self.ollaya_api_key.as_ref(),
        }?;
        Some(key.as_str())
    }

    pub fn is_local(&self) -> bool {
        self.provider == DecisionModelsProvider::Ollaya
    }
}

#[cfg(test)]
#[path = "decision_models_tests.rs"]
mod tests;
