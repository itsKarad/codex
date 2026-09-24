use std::num::NonZeroUsize;

use codex_config::types::RedactedString;

/// Host-supplied configuration used by the skills extension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillsExtensionConfig {
    /// Whether the available-skills catalog is included in model context.
    pub include_instructions: bool,
    /// Optional token budget override for the available-skills catalog.
    pub max_context_tokens: Option<NonZeroUsize>,
    /// Whether bundled skills are eligible for discovery.
    pub bundled_skills_enabled: bool,
    /// Whether cloud skills are discovered and exposed to the model.
    pub cloud_skill_enabled: bool,
    /// Whether cheap skill selectors run in shadow mode without changing prompt contents.
    pub shadow_selection_enabled: bool,
    /// Whether Jev may suggest high-confidence skills for substantive turns.
    pub jev_skill_selection_enabled: bool,
    /// OpenRouter API key for Jev features, loaded from `[tui].jev_openrouter_api_key`.
    pub jev_openrouter_api_key: Option<RedactedString>,
}
