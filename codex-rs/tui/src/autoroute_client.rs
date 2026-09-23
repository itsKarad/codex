//! OpenRouter Decisions API client for the TUI's model routing flow.
//!
//! The caller supplies a route-aware HTTP pool and credentials so this client uses the same
//! outbound network policy as the rest of the CLI. The API key is never stored in this module.

use std::collections::BTreeMap;
use std::time::Duration;

use codex_http_client::RouteAwareClientPool;
use serde::Deserialize;
use serde_json::Value;

const OPENROUTER_DECISIONS_URL: &str = "https://openrouter.ai/api/alpha/decisions";
pub(crate) const DEFAULT_JEV_MODEL: &str = "~typesafe/jev-latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TASK_CHARS: usize = 16_000;
const MAX_HISTORY_CHARS: usize = 4_000;
const MAX_CANDIDATES: usize = 64;
const MAX_ATTACHMENTS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 1_000;
const MAX_MODALITIES: usize = 16;
const MAX_MOD_CHARACTERS: usize = 128;
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// The price preference used by the first Jev decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RoutingMode {
    CostEffective,
    StrongestFit,
}

impl RoutingMode {
    fn instructions(self) -> &'static str {
        match self {
            Self::CostEffective => {
                "Choose the least expensive model that can reliably complete the task. Treat unknown prices as unknown, never as free or cheap."
            }
            Self::StrongestFit => {
                "Prioritize the model most capable of completing the task. Use price only to break close capability ties. Treat unknown prices as unknown, never as free or cheap."
            }
        }
    }
}

/// A model currently visible in the active Codex model picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelCandidate {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_modalities: Vec<String>,
    pub(crate) supported_efforts: Vec<String>,
}

/// Non-content metadata describing attached inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttachmentMetadata {
    /// A broad type such as `image`, `audio`, `document`, or `other`.
    pub(crate) kind: String,
    pub(crate) count: usize,
}

/// Routing inputs assembled by the TUI. `recent_history` should contain only user/reply pairs.
pub(crate) struct RoutingRequest<'a> {
    pub(crate) mode: RoutingMode,
    pub(crate) task: &'a str,
    pub(crate) recent_history: Option<&'a str>,
    pub(crate) attachments: &'a [AttachmentMetadata],
    pub(crate) candidates: &'a [ModelCandidate],
}

/// A validated model and reasoning-effort recommendation from Jev.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RouteRecommendation {
    pub(crate) model: String,
    pub(crate) effort: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AutoRouteError {
    #[error("OPENROUTER_API_KEY is not set")]
    MissingApiKey,
    #[error("no available models were provided to Jev")]
    NoCandidates,
    #[error("the model catalog contains duplicate model slugs")]
    DuplicateCandidate,
    #[error("the model catalog exceeds the supported size")]
    TooManyCandidates,
    #[error("Jev request exceeds the 65536-byte size limit")]
    RequestTooLarge,
    #[error("request to OpenRouter failed: {0}")]
    Request(#[from] codex_http_client::RouteAwareRequestError),
    #[error("OpenRouter returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("OpenRouter returned an invalid JSON response")]
    InvalidResponse(#[source] codex_http_client::HttpError),
    #[error("Jev's response did not include a {0} choice")]
    MissingChoice(&'static str),
    #[error("model `{0}` does not expose any supported reasoning efforts")]
    NoSupportedEfforts(String),
    #[error("Jev selected a model that is not in the active model picker: {0}")]
    UnknownModel(String),
    #[error("Jev selected unsupported reasoning effort `{effort}` for model `{model}`")]
    UnsupportedEffort { model: String, effort: String },
}

/// Uses OpenRouter's Decisions API to select a model and then a supported effort.
///
/// The two choices are deliberately requested in sequence: the effort options in the second
/// request are taken from the validated model selected in the first response.
pub(crate) async fn recommend_route(
    client: &RouteAwareClientPool,
    api_key: Option<&str>,
    jev_model: &str,
    request: &RoutingRequest<'_>,
) -> Result<RouteRecommendation, AutoRouteError> {
    let api_key = api_key
        .filter(|key| !key.trim().is_empty())
        .ok_or(AutoRouteError::MissingApiKey)?;
    validate_catalog(request.candidates)?;

    let model_request = build_model_request(jev_model, request)?;
    let model_choice = send_choice_request(client, api_key, model_request, "model").await?;
    let model = validate_model_choice(&model_choice, request.candidates)?;

    let effort_request = build_effort_request(jev_model, request, model)?;
    let effort = send_choice_request(client, api_key, effort_request, "effort").await?;
    validate_effort_choice(model, &effort)?;

    Ok(RouteRecommendation {
        model: model.slug.clone(),
        effort,
    })
}

/// Selects the configured Jev model without reading process environment state here.
pub(crate) fn jev_model_from_configured_value(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_JEV_MODEL)
        .to_string()
}

fn validate_catalog(candidates: &[ModelCandidate]) -> Result<(), AutoRouteError> {
    if candidates.is_empty() {
        return Err(AutoRouteError::NoCandidates);
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(AutoRouteError::TooManyCandidates);
    }
    for (index, candidate) in candidates.iter().enumerate() {
        if candidates[..index]
            .iter()
            .any(|prior| prior.slug == candidate.slug)
        {
            return Err(AutoRouteError::DuplicateCandidate);
        }
    }
    Ok(())
}

fn build_model_request(
    jev_model: &str,
    routing: &RoutingRequest<'_>,
) -> Result<Value, AutoRouteError> {
    validate_catalog(routing.candidates)?;
    let choices = routing
        .candidates
        .iter()
        .map(|candidate| (candidate.slug.clone(), candidate_description(candidate)))
        .collect::<BTreeMap<_, _>>();
    let state = routing_state(routing);
    let request = serde_json::json!({
        "model": jev_model,
        "questions": {
            "model": {
                "type": "choice",
                "instructions": routing.mode.instructions(),
                "criteria": choices,
            }
        },
        "state": state,
    });
    ensure_request_size(&request)?;
    Ok(request)
}

fn build_effort_request(
    jev_model: &str,
    routing: &RoutingRequest<'_>,
    candidate: &ModelCandidate,
) -> Result<Value, AutoRouteError> {
    let choices = candidate
        .supported_efforts
        .iter()
        .map(|effort| {
            (
                effort.clone(),
                format!("Use the `{effort}` reasoning effort for this task."),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if choices.is_empty() {
        return Err(AutoRouteError::NoSupportedEfforts(candidate.slug.clone()));
    }
    let mut state = routing_state(routing);
    state["selected_model"] = serde_json::json!({
        "slug": &candidate.slug,
        "name": &candidate.name,
        "description": truncate_chars(&candidate.description, MAX_DESCRIPTION_CHARS),
        "supported_reasoning_efforts": &candidate.supported_efforts,
    });
    let request = serde_json::json!({
        "model": jev_model,
        "questions": {
            "effort": {
                "type": "choice",
                "instructions": "Choose the least reasoning effort that is likely to complete the task reliably.",
                "criteria": choices,
            }
        },
        "state": state,
    });
    ensure_request_size(&request)?;
    Ok(request)
}

fn routing_state(routing: &RoutingRequest<'_>) -> Value {
    let task = truncate_chars(routing.task, MAX_TASK_CHARS);
    let history = routing
        .recent_history
        .map(|history| truncate_chars(history, MAX_HISTORY_CHARS));
    let attachments = routing
        .attachments
        .iter()
        .take(MAX_ATTACHMENTS)
        .map(|attachment| {
            serde_json::json!({
                "type": truncate_chars(&attachment.kind, MAX_MOD_CHARACTERS),
                "count": attachment.count,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "task": task,
        "recent_user_reply_history": history,
        "attachments": attachments,
    })
}

fn candidate_description(candidate: &ModelCandidate) -> String {
    let modalities = candidate
        .input_modalities
        .iter()
        .take(MAX_MODALITIES)
        .map(|modality| truncate_chars(modality, MAX_MOD_CHARACTERS))
        .collect::<Vec<_>>();
    let efforts = candidate
        .supported_efforts
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let description = truncate_chars(&candidate.description, MAX_DESCRIPTION_CHARS);
    let pricing = model_pricing(&candidate.slug).map_or_else(
        || "standard OpenAI API price unknown; do not treat as low cost".to_string(),
        |(input, output)| {
            format!(
                "standard OpenAI API price: ${input}/million input tokens and ${output}/million output tokens (routing signal only)"
            )
        },
    );
    format!(
        "{} ({}) — {}; input modalities: {}; supported reasoning efforts: {}; {}.",
        candidate.name,
        candidate.slug,
        description,
        if modalities.is_empty() {
            "unspecified".to_string()
        } else {
            modalities.join(", ")
        },
        if efforts.is_empty() {
            "unspecified".to_string()
        } else {
            efforts.join(", ")
        },
        pricing,
    )
}

fn model_pricing(slug: &str) -> Option<(f64, f64)> {
    match slug {
        "gpt-6-astra" => Some((10.0, 50.0)),
        "gpt-6-sol" => Some((2.0, 10.0)),
        "gpt-6-luna" => Some((0.10, 0.50)),
        "gpt-5.6-sol" => Some((4.0, 20.0)),
        "gpt-5.6-terra" => Some((2.0, 12.0)),
        "gpt-5.6-luna" => Some((0.20, 1.20)),
        "gpt-5.5" => Some((5.0, 30.0)),
        _ => None,
    }
}

async fn send_choice_request(
    client: &RouteAwareClientPool,
    api_key: &str,
    request: Value,
    question: &'static str,
) -> Result<String, AutoRouteError> {
    let response = client
        .post(OPENROUTER_DECISIONS_URL)
        .bearer_auth(api_key)
        .timeout(REQUEST_TIMEOUT)
        .json(&request)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(AutoRouteError::HttpStatus(status.as_u16()));
    }
    let response = response
        .json::<DecisionResponse>()
        .await
        .map_err(AutoRouteError::InvalidResponse)?;
    parse_choice_answer(&response, question)
}

fn parse_choice_answer(
    response: &DecisionResponse,
    question: &'static str,
) -> Result<String, AutoRouteError> {
    response
        .answers
        .get(question)
        .and_then(|answer| answer.choice.as_deref())
        .filter(|choice| !choice.is_empty())
        .map(str::to_owned)
        .ok_or(AutoRouteError::MissingChoice(question))
}

fn validate_model_choice<'a>(
    choice: &str,
    candidates: &'a [ModelCandidate],
) -> Result<&'a ModelCandidate, AutoRouteError> {
    candidates
        .iter()
        .find(|candidate| candidate.slug == choice)
        .ok_or_else(|| AutoRouteError::UnknownModel(choice.to_string()))
}

fn validate_effort_choice(candidate: &ModelCandidate, effort: &str) -> Result<(), AutoRouteError> {
    if candidate
        .supported_efforts
        .iter()
        .any(|supported| supported == effort)
    {
        return Ok(());
    }
    Err(AutoRouteError::UnsupportedEffort {
        model: candidate.slug.clone(),
        effort: effort.to_string(),
    })
}

#[derive(Deserialize)]
struct DecisionResponse {
    answers: BTreeMap<String, DecisionAnswer>,
}

#[derive(Deserialize)]
struct DecisionAnswer {
    #[serde(default)]
    choice: Option<String>,
}

fn ensure_request_size(request: &Value) -> Result<(), AutoRouteError> {
    if serde_json::to_vec(request)
        .map_err(|_| AutoRouteError::RequestTooLarge)?
        .len()
        > MAX_REQUEST_BYTES
    {
        return Err(AutoRouteError::RequestTooLarge);
    }
    Ok(())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "autoroute_client_tests.rs"]
mod tests;
