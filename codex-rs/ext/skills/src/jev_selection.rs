//! High-confidence skill suggestions produced with OpenRouter's Jev Decisions API.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;

use codex_http_client::RouteAwareClientPool;
use codex_protocol::user_input::UserInput;
use codex_skills::extract_tool_mentions;
use serde::Deserialize;
use serde_json::Value;

use crate::catalog::SkillCatalogEntry;
use crate::dynamic_skill_selector::CheapSkillSelector;
use crate::dynamic_skill_selector::SkillSelectionDocument;
use crate::dynamic_skill_selector::WeightedLexicalSkillSelector;
use crate::selection::entry_matches_explicit_name;

const OPENROUTER_DECISIONS_URL: &str = "https://openrouter.ai/api/alpha/decisions";
const DEFAULT_JEV_MODEL: &str = "~typesafe/jev-latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_TASK_CHARS: usize = 12_000;
const MAX_RECENT_REQUEST_CHARS: usize = 2_000;
const MAX_SKILL_NAME_CHARS: usize = 128;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 600;
const MAX_WIDE_CANDIDATES: usize = 48;
const MAX_SHORTLIST: usize = 12;
const MAX_SUGGESTIONS: usize = 3;
const MIN_FIT_PROBABILITY: f64 = 0.85;

/// Bounded user task context used only for Jev skill selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillSuggestionContext {
    pub(crate) request: String,
    pub(crate) recent_requests: Vec<String>,
}

/// One distinct skill that passed Jev's independent fit check.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JevSkillSuggestion {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) fit_probability: f64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum JevSkillSelectionError {
    #[error("tui.jev_openrouter_api_key is not set in config.toml")]
    MissingApiKey,
    #[error("Jev skill-selection request exceeds the supported size")]
    RequestTooLarge,
    #[error("request to OpenRouter failed: {0}")]
    Request(#[from] codex_http_client::RouteAwareRequestError),
    #[error("OpenRouter returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("OpenRouter returned an invalid Jev response")]
    InvalidResponse(#[source] codex_http_client::HttpError),
    #[error("Jev returned a missing or invalid {0} answer")]
    InvalidAnswer(String),
}

#[derive(Clone)]
pub(crate) struct JevSkillSelector {
    client: RouteAwareClientPool,
    endpoint: String,
}

impl JevSkillSelector {
    pub(crate) fn new(client: RouteAwareClientPool) -> Self {
        Self {
            client,
            endpoint: OPENROUTER_DECISIONS_URL.to_string(),
        }
    }

    pub(crate) async fn suggest(
        &self,
        api_key: Option<&str>,
        model: Option<&str>,
        context: &SkillSuggestionContext,
        entries: &[SkillCatalogEntry],
    ) -> Result<Vec<JevSkillSuggestion>, JevSkillSelectionError> {
        let candidates = eligible_candidates(context, entries);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let api_key = api_key
            .map(str::trim)
            .filter(|key| !key.trim().is_empty())
            .ok_or(JevSkillSelectionError::MissingApiKey)?;

        let model = model
            .map(str::trim)
            .filter(|model| !model.trim().is_empty())
            .unwrap_or(DEFAULT_JEV_MODEL);
        let state = build_state(context, &candidates);
        let ranking_request = build_ranking_request(model, &state, &candidates)?;
        let ranking_response = self.send(api_key, &ranking_request).await?;
        let shortlist = parse_ranking(&ranking_response, &candidates)?;

        let verification_state = build_state(context, &shortlist);
        let verification_request =
            build_verification_request(model, &verification_state, &shortlist)?;
        let verification_response = self.send(api_key, &verification_request).await?;
        parse_verification(&verification_response, &shortlist)
    }

    async fn send(
        &self,
        api_key: &str,
        request: &Value,
    ) -> Result<DecisionResponse, JevSkillSelectionError> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(api_key)
            .timeout(REQUEST_TIMEOUT)
            .json(request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(JevSkillSelectionError::HttpStatus(status.as_u16()));
        }
        response
            .json::<DecisionResponse>()
            .await
            .map_err(JevSkillSelectionError::InvalidResponse)
    }

    #[cfg(test)]
    fn with_endpoint(client: RouteAwareClientPool, endpoint: String) -> Self {
        Self { client, endpoint }
    }
}

#[derive(Clone, Debug)]
struct SkillCandidate {
    entry_index: usize,
    key: String,
    name: String,
    source: String,
    path: String,
    description: String,
}

#[derive(Deserialize)]
struct DecisionResponse {
    answers: HashMap<String, DecisionAnswer>,
}

#[derive(Deserialize)]
struct DecisionAnswer {
    #[serde(rename = "type")]
    answer_type: String,
    choice: Option<String>,
    probabilities: Option<HashMap<String, f64>>,
    noul: Option<f64>,
}

pub(crate) fn configured_model(value: Option<&str>) -> &str {
    value
        .map(str::trim)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or(DEFAULT_JEV_MODEL)
}

pub(crate) fn substantive_request(inputs: &[UserInput]) -> Option<String> {
    let text = inputs
        .iter()
        .filter_map(|input| match input {
            UserInput::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = truncate_chars(&text, MAX_TASK_CHARS);
    let normalized = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    (!matches!(
        normalized.as_str(),
        "" | "yes"
            | "yep"
            | "yeah"
            | "ok"
            | "okay"
            | "sure"
            | "go"
            | "go ahead"
            | "continue"
            | "please continue"
            | "proceed"
            | "do it"
            | "do that"
            | "try again"
            | "retry"
            | "thanks"
            | "thank you"
    ))
    .then_some(text)
}

pub(crate) fn has_explicit_skill_selection(
    inputs: &[UserInput],
    entries: &[SkillCatalogEntry],
) -> bool {
    inputs.iter().any(|input| match input {
        UserInput::Skill { .. } => true,
        UserInput::Mention { path, .. } => is_skill_path(path),
        UserInput::Text { text, .. } => {
            let mentions = extract_tool_mentions(text);
            mentions.paths().any(is_skill_path)
                || mentions.plain_names().any(|name| {
                    entries
                        .iter()
                        .any(|entry| entry_matches_explicit_name(entry, name))
                })
        }
        _ => false,
    })
}

fn is_skill_path(path: &str) -> bool {
    path.starts_with("skill://")
        || path
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
}

fn eligible_candidates(
    context: &SkillSuggestionContext,
    entries: &[SkillCatalogEntry],
) -> Vec<SkillCandidate> {
    let mut seen = HashSet::new();
    let mut candidates = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.enabled && entry.prompt_visible)
        .filter(|(_, entry)| seen.insert((entry.authority.clone(), entry.id.clone())))
        .map(|(entry_index, entry)| SkillCandidate {
            entry_index,
            key: format!("skill_{entry_index}"),
            name: truncate_chars(&entry.name, MAX_SKILL_NAME_CHARS),
            source: truncate_chars(&entry.authority.kind.to_string(), 48),
            path: truncate_chars(entry.rendered_path(), MAX_SKILL_NAME_CHARS * 2),
            description: truncate_chars(&entry.description, MAX_SKILL_DESCRIPTION_CHARS),
        })
        .collect::<Vec<_>>();

    if candidates.len() <= MAX_WIDE_CANDIDATES {
        return candidates;
    }

    let documents = candidates
        .iter()
        .map(|candidate| {
            let entry = &entries[candidate.entry_index];
            SkillSelectionDocument {
                id: candidate.entry_index,
                name: &entry.name,
                short_description: entry.short_description.as_deref(),
                description: &entry.description,
                dependencies: entry.dependencies.as_ref(),
            }
        })
        .collect::<Vec<_>>();
    let selected = WeightedLexicalSkillSelector
        .select(&context.request, &documents, MAX_WIDE_CANDIDATES)
        .candidate_ids
        .into_iter()
        .collect::<HashSet<_>>();
    candidates.retain(|candidate| selected.contains(&candidate.entry_index));
    candidates
}

fn build_state(context: &SkillSuggestionContext, candidates: &[SkillCandidate]) -> Value {
    serde_json::json!({
        "request": truncate_chars(&context.request, MAX_TASK_CHARS),
        "recent_requests": context.recent_requests.iter()
            .take(2)
            .map(|request| truncate_chars(request, MAX_RECENT_REQUEST_CHARS))
            .collect::<Vec<_>>(),
        "skills": candidates.iter().map(|candidate| serde_json::json!({
            "id": candidate.key,
            "name": candidate.name,
            "source": candidate.source,
            "description": candidate.description,
        })).collect::<Vec<_>>(),
    })
}

fn build_ranking_request(
    model: &str,
    state: &Value,
    candidates: &[SkillCandidate],
) -> Result<Value, JevSkillSelectionError> {
    let criteria = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.key.clone(),
                serde_json::json!({
                    "name": candidate.name,
                    "source": candidate.source,
                    "description": candidate.description,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let request = serde_json::json!({
        "model": model,
        "state": state,
        "questions": {
            "rank": {
                "type": "choice",
                "instructions": "Which skills, if any, are directly relevant to completing the user's current request? Rank by usefulness, and prefer no-match over a merely related topic.",
                "criteria": criteria,
            }
        }
    });
    ensure_request_size(&request)?;
    Ok(request)
}

fn build_verification_request(
    model: &str,
    state: &Value,
    candidates: &[SkillCandidate],
) -> Result<Value, JevSkillSelectionError> {
    let questions = candidates
        .iter()
        .map(|candidate| {
            (
                format!("fit_{}", candidate.key),
                serde_json::json!({
                    "type": "noul",
                    "instructions": format!(
                        "Would the skill described by `skills` entry `{}` materially help complete the user's current request? Answer yes only when its documented workflow directly applies; related subject matter alone is not a fit.",
                        candidate.key,
                    ),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let request = serde_json::json!({
        "model": model,
        "state": state,
        "questions": questions,
    });
    ensure_request_size(&request)?;
    Ok(request)
}

fn ensure_request_size(request: &Value) -> Result<(), JevSkillSelectionError> {
    if request.to_string().len() > MAX_REQUEST_BYTES {
        return Err(JevSkillSelectionError::RequestTooLarge);
    }
    Ok(())
}

fn parse_ranking(
    response: &DecisionResponse,
    candidates: &[SkillCandidate],
) -> Result<Vec<SkillCandidate>, JevSkillSelectionError> {
    let answer = response
        .answers
        .get("rank")
        .filter(|answer| answer.answer_type == "choice")
        .ok_or_else(|| JevSkillSelectionError::InvalidAnswer("rank".to_string()))?;
    let choice = answer
        .choice
        .as_deref()
        .filter(|choice| candidates.iter().any(|candidate| candidate.key == *choice))
        .ok_or_else(|| JevSkillSelectionError::InvalidAnswer("rank".to_string()))?;
    let probabilities = answer
        .probabilities
        .as_ref()
        .ok_or_else(|| JevSkillSelectionError::InvalidAnswer("rank".to_string()))?;
    if !probabilities.contains_key(choice)
        || probabilities
            .values()
            .any(|probability| !probability.is_finite() || !(0.0..=1.0).contains(probability))
    {
        return Err(JevSkillSelectionError::InvalidAnswer("rank".to_string()));
    }
    let mut ranked = probabilities
        .iter()
        .filter_map(|(key, probability)| {
            candidates
                .iter()
                .find(|candidate| candidate.key == *key)
                .map(|candidate| (candidate, *probability))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left, left_probability), (right, right_probability)| {
        right_probability
            .total_cmp(left_probability)
            .then_with(|| left.key.cmp(&right.key))
    });
    Ok(ranked
        .into_iter()
        .take(MAX_SHORTLIST)
        .map(|(candidate, _)| candidate.clone())
        .collect())
}

fn parse_verification(
    response: &DecisionResponse,
    shortlist: &[SkillCandidate],
) -> Result<Vec<JevSkillSuggestion>, JevSkillSelectionError> {
    let mut verified = Vec::new();
    for candidate in shortlist {
        let question_id = format!("fit_{}", candidate.key);
        let probability = response
            .answers
            .get(&question_id)
            .filter(|answer| answer.answer_type == "noul")
            .and_then(|answer| answer.noul)
            .filter(|probability| probability.is_finite() && (0.0..=1.0).contains(probability))
            .ok_or_else(|| JevSkillSelectionError::InvalidAnswer(question_id.clone()))?;
        if probability >= MIN_FIT_PROBABILITY {
            verified.push(JevSkillSuggestion {
                name: candidate.name.clone(),
                path: candidate.path.clone(),
                fit_probability: probability,
            });
        }
    }
    verified.sort_by(|left, right| {
        right
            .fit_probability
            .total_cmp(&left.fit_probability)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut seen_names = HashSet::new();
    let mut seen_paths = HashSet::new();
    verified.retain(|suggestion| {
        seen_names.insert(suggestion.name.trim().to_lowercase())
            && seen_paths.insert(suggestion.path.clone())
    });
    verified.truncate(MAX_SUGGESTIONS);
    Ok(verified)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut result = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

#[derive(Default)]
pub(crate) struct SkillSuggestionHistory(std::sync::Mutex<SkillSuggestionHistoryState>);

#[derive(Default)]
struct SkillSuggestionHistoryState {
    prior_requests: std::collections::VecDeque<(String, String)>,
    current: Option<(String, String)>,
}

impl SkillSuggestionHistory {
    pub(crate) fn context_for_turn(&self, turn_id: &str, request: &str) -> SkillSuggestionContext {
        let request = truncate_chars(request, MAX_TASK_CHARS);
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .current
            .as_ref()
            .is_none_or(|(current_id, _)| current_id != turn_id)
        {
            if let Some(previous) = state
                .current
                .replace((turn_id.to_string(), request.clone()))
            {
                state
                    .prior_requests
                    .retain(|(prior_id, _)| prior_id != &previous.0);
                state.prior_requests.push_front(previous);
                state.prior_requests.truncate(2);
            } else {
                state.current = Some((turn_id.to_string(), request.clone()));
            }
        }
        let recent_requests = state
            .prior_requests
            .iter()
            .rev()
            .map(|(_, request)| request.clone())
            .collect();
        SkillSuggestionContext {
            request,
            recent_requests,
        }
    }
}

#[cfg(test)]
#[path = "jev_selection_tests.rs"]
mod tests;
