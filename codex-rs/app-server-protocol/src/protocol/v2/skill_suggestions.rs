use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

/// Status of the configured decision provider's advisory skill-selection pass for a turn.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SkillSuggestionNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub status: SkillSuggestionStatus,
    /// Actionable setup hint when the decision provider was unavailable.
    pub error_message: Option<String>,
    pub suggestions: Vec<SkillSuggestion>,
}

/// Whether the decision provider returned strong skill matches, no match, or could not complete the check.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum SkillSuggestionStatus {
    Suggested,
    NoMatch,
    Unavailable,
}

/// A skill rated as a high-confidence fit for the current request.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SkillSuggestion {
    pub name: String,
    pub fit_probability: f64,
}
