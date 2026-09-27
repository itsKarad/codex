//! High-confidence skill suggestions from the configured decision model provider.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;

use codex_config::DecisionModelsConfig;
use codex_config::DecisionModelsProvider;
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

const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const OLLAYA_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_TASK_CHARS: usize = 12_000;
const MAX_RECENT_REQUEST_CHARS: usize = 2_000;
const MAX_SKILL_NAME_CHARS: usize = 128;
const MAX_SKILL_DESCRIPTION_CHARS: usize = 600;
const MAX_WIDE_CANDIDATES: usize = 48;
const MAX_SHORTLIST: usize = 12;
const MAX_SUGGESTIONS: usize = 3;
const MIN_FIT_PROBABILITY: f64 = 0.85;
const MAX_ERROR_MESSAGE_CHARS: usize = 256;

/// Bounded user task context used only for decision-model skill selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillSuggestionContext {
    pub(crate) request: String,
    pub(crate) recent_requests: Vec<String>,
}

/// One distinct skill that passed the independent fit check.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JevSkillSuggestion {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) fit_probability: f64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum JevSkillSelectionError {
    #[error("decision_models.openrouter_api_key is not set in config.toml")]
    MissingApiKey,
    #[error(
        "could not reach Ollaya; check decision_models.ollaya_base_url and start it with `ollaya serve`"
    )]
    OllayaUnavailable,
    #[error("Ollaya model `{0}` is not installed; run `ollaya pull {0}`")]
    OllayaModelNotFound(String),
    #[error("Ollaya rejected its bearer key; check decision_models.ollaya_api_key")]
    OllayaUnauthorized,
    #[error("Ollaya request timed out after 15 seconds")]
    OllayaTimeout,
    #[error("Ollaya returned HTTP status {0}")]
    OllayaHttpStatus(u16),
    #[error("Ollaya returned an invalid decision response")]
    OllayaInvalidResponse(#[source] codex_http_client::HttpError),
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

impl JevSkillSelectionError {
    pub(crate) fn user_message(&self) -> String {
        let message = self
            .to_string()
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>();
        truncate_chars(&message, MAX_ERROR_MESSAGE_CHARS)
    }
}

#[derive(Clone)]
pub(crate) struct JevSkillSelector {
    client: RouteAwareClientPool,
    test_endpoint: Option<String>,
}

impl JevSkillSelector {
    pub(crate) fn new(client: RouteAwareClientPool) -> Self {
        Self {
            client,
            test_endpoint: None,
        }
    }

    pub(crate) async fn suggest(
        &self,
        decision_models: &DecisionModelsConfig,
        openrouter_model: Option<&str>,
        context: &SkillSuggestionContext,
        entries: &[SkillCatalogEntry],
    ) -> Result<Vec<JevSkillSuggestion>, JevSkillSelectionError> {
        if decision_models.is_local() {
            tokio::time::timeout(
                OLLAYA_TOTAL_TIMEOUT,
                self.suggest_with_timeout(
                    decision_models,
                    openrouter_model,
                    context,
                    entries,
                    OLLAYA_TOTAL_TIMEOUT,
                ),
            )
            .await
            .map_err(|_| JevSkillSelectionError::OllayaTimeout)?
        } else {
            self.suggest_with_timeout(
                decision_models,
                openrouter_model,
                context,
                entries,
                REQUEST_TIMEOUT,
            )
            .await
        }
    }

    async fn suggest_with_timeout(
        &self,
        decision_models: &DecisionModelsConfig,
        openrouter_model: Option<&str>,
        context: &SkillSuggestionContext,
        entries: &[SkillCatalogEntry],
        timeout: Duration,
    ) -> Result<Vec<JevSkillSuggestion>, JevSkillSelectionError> {
        let candidates = eligible_candidates(context, entries);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let local = decision_models.provider == DecisionModelsProvider::Ollaya;
        let api_key = decision_models
            .api_key()
            .map(str::trim)
            .filter(|key| !key.is_empty());
        if !local && api_key.is_none() {
            return Err(JevSkillSelectionError::MissingApiKey);
        }
        let endpoint = self
            .test_endpoint
            .clone()
            .unwrap_or_else(|| decision_models.endpoint());
        let model = decision_models.model(openrouter_model);
        let state = build_state(context, &candidates);
        let ranking_request = build_ranking_request(&model, &state, &candidates, local)?;
        let ranking_response = self
            .send(
                decision_models,
                &endpoint,
                api_key,
                &model,
                &ranking_request,
                timeout,
            )
            .await?;
        let shortlist = parse_ranking(&ranking_response, &candidates, local)?;
        if shortlist.is_empty() {
            return Ok(Vec::new());
        }

        let verification_state = build_state(context, &shortlist);
        let verification_request =
            build_verification_request(&model, &verification_state, &shortlist)?;
        let verification_response = self
            .send(
                decision_models,
                &endpoint,
                api_key,
                &model,
                &verification_request,
                timeout,
            )
            .await?;
        parse_verification(&verification_response, &shortlist)
    }

    async fn send(
        &self,
        decision_models: &DecisionModelsConfig,
        endpoint: &str,
        api_key: Option<&str>,
        model: &str,
        request: &Value,
        timeout: Duration,
    ) -> Result<DecisionResponse, JevSkillSelectionError> {
        let mut builder = self.client.post(endpoint).timeout(timeout).json(request);
        if let Some(api_key) = api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = match builder.send().await {
            Ok(response) => response,
            Err(_error) if decision_models.is_local() => {
                return Err(JevSkillSelectionError::OllayaUnavailable);
            }
            Err(error) => return Err(JevSkillSelectionError::Request(error)),
        };
        let status = response.status();
        if !status.is_success() {
            if decision_models.is_local() {
                let code = response
                    .text()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                    .and_then(|body| body.get("code").and_then(Value::as_str).map(str::to_string));
                return match (status.as_u16(), code.as_deref()) {
                    (401 | 403, _) => Err(JevSkillSelectionError::OllayaUnauthorized),
                    (404, Some("MODEL_NOT_FOUND")) => Err(
                        JevSkillSelectionError::OllayaModelNotFound(model.to_string()),
                    ),
                    (status, _) => Err(JevSkillSelectionError::OllayaHttpStatus(status)),
                };
            }
            return Err(JevSkillSelectionError::HttpStatus(status.as_u16()));
        }
        response.json::<DecisionResponse>().await.map_err(|error| {
            if decision_models.is_local() {
                JevSkillSelectionError::OllayaInvalidResponse(error)
            } else {
                JevSkillSelectionError::InvalidResponse(error)
            }
        })
    }

    #[cfg(test)]
    fn with_endpoint(client: RouteAwareClientPool, endpoint: String) -> Self {
        Self {
            client,
            test_endpoint: Some(endpoint),
        }
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
    include_no_match: bool,
) -> Result<Value, JevSkillSelectionError> {
    let mut criteria = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.key.clone(),
                if include_no_match {
                    serde_json::json!(format!(
                        "{}; source: {}; description: {}",
                        candidate.name, candidate.source, candidate.description
                    ))
                } else {
                    serde_json::json!({
                        "name": candidate.name,
                        "source": candidate.source,
                        "description": candidate.description,
                    })
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    if include_no_match {
        criteria.insert(
            "no_match".to_string(),
            serde_json::json!("No listed skill directly applies to the user's request."),
        );
    }
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
    allow_no_match: bool,
) -> Result<Vec<SkillCandidate>, JevSkillSelectionError> {
    let answer = response
        .answers
        .get("rank")
        .filter(|answer| answer.answer_type == "choice")
        .ok_or_else(|| JevSkillSelectionError::InvalidAnswer("rank".to_string()))?;
    let choice = answer
        .choice
        .as_deref()
        .filter(|choice| {
            (allow_no_match && *choice == "no_match")
                || candidates.iter().any(|candidate| candidate.key == *choice)
        })
        .ok_or_else(|| JevSkillSelectionError::InvalidAnswer("rank".to_string()))?;
    if allow_no_match && choice == "no_match" {
        return Ok(Vec::new());
    }
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
        result.pop();
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
