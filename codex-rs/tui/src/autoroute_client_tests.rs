use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn candidate(slug: &str) -> ModelCandidate {
    ModelCandidate {
        slug: slug.to_string(),
        name: slug.to_string(),
        description: format!("Description of {slug}"),
        input_modalities: vec!["text".to_string(), "image".to_string()],
        supported_efforts: vec!["low".to_string(), "high".to_string()],
    }
}

fn routing_request<'a>(
    task: &'a str,
    history: Option<&'a str>,
    attachments: &'a [AttachmentMetadata],
    candidates: &'a [ModelCandidate],
    mode: RoutingMode,
) -> RoutingRequest<'a> {
    RoutingRequest {
        mode,
        task,
        recent_history: history,
        attachments,
        candidates,
    }
}

#[test]
fn model_request_has_mode_context_catalog_and_attachment_metadata_only() {
    let attachments = vec![AttachmentMetadata {
        kind: "image".to_string(),
        count: 2,
    }];
    let candidates = vec![candidate("gpt-6-sol"), candidate("future-model")];
    let routing = routing_request(
        "Implement a parser",
        Some("User: use Rust\nAssistant: I'll inspect the grammar."),
        &attachments,
        &candidates,
        RoutingMode::CostEffective,
    );

    let request = build_model_request("~typesafe/jev-latest", &routing).expect("request");

    assert_eq!(
        request,
        json!({
            "model": "~typesafe/jev-latest",
            "questions": {
                "model": {
                    "type": "choice",
                    "instructions": "Choose the least expensive model that can reliably complete the task. Treat unknown prices as unknown, never as free or cheap.",
                    "criteria": {
                        "future-model": "future-model (future-model) — Description of future-model; input modalities: text, image; supported reasoning efforts: low, high; standard OpenAI API price unknown; do not treat as low cost.",
                        "gpt-6-sol": "gpt-6-sol (gpt-6-sol) — Description of gpt-6-sol; input modalities: text, image; supported reasoning efforts: low, high; standard OpenAI API price: $2/million input tokens and $10/million output tokens (routing signal only)."
                    }
                }
            },
            "state": {
                "task": "Implement a parser",
                "recent_user_reply_history": "User: use Rust\nAssistant: I'll inspect the grammar.",
                "attachments": [{"type": "image", "count": 2}]
            }
        })
    );
    assert!(!request.to_string().contains("attachment contents"));
}

#[test]
fn effort_request_is_a_typed_choice_for_only_the_selected_models_efforts() {
    let attachments = [];
    let candidates = vec![candidate("gpt-6-sol")];
    let routing = routing_request(
        "Fix a bug",
        None,
        &attachments,
        &candidates,
        RoutingMode::StrongestFit,
    );

    let request = build_effort_request("jev", &routing, &candidates[0]).expect("request");

    assert_eq!(
        request,
        json!({
            "model": "jev",
            "questions": {
                "effort": {
                    "type": "choice",
                    "instructions": "Choose the least reasoning effort that is likely to complete the task reliably.",
                    "criteria": {
                        "high": "Use the `high` reasoning effort for this task.",
                        "low": "Use the `low` reasoning effort for this task."
                    }
                }
            },
            "state": {
                "task": "Fix a bug",
                "recent_user_reply_history": null,
                "attachments": [],
                "selected_model": {
                    "slug": "gpt-6-sol",
                    "name": "gpt-6-sol",
                    "description": "Description of gpt-6-sol",
                    "supported_reasoning_efforts": ["low", "high"]
                }
            }
        })
    );
}

#[test]
fn all_seven_known_models_have_the_expected_standard_api_prices() {
    assert_eq!(
        [
            ("gpt-6-astra", Some((10.0, 50.0))),
            ("gpt-6-sol", Some((2.0, 10.0))),
            ("gpt-6-luna", Some((0.10, 0.50))),
            ("gpt-5.6-sol", Some((4.0, 20.0))),
            ("gpt-5.6-terra", Some((2.0, 12.0))),
            ("gpt-5.6-luna", Some((0.20, 1.20))),
            ("gpt-5.5", Some((5.0, 30.0))),
            ("future-model", None),
        ],
        [
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "future-model",
        ]
        .map(|slug| (slug, model_pricing(slug)))
    );
}

#[test]
fn unknown_price_does_not_exclude_a_model_from_choices() {
    let attachments = [];
    let candidates = vec![candidate("future-model")];
    let routing = routing_request(
        "Write a quick summary",
        None,
        &attachments,
        &candidates,
        RoutingMode::CostEffective,
    );

    let request = build_model_request("jev", &routing).expect("unknown-price model stays eligible");

    assert_eq!(
        request["questions"]["model"]["criteria"]["future-model"],
        "future-model (future-model) — Description of future-model; input modalities: text, image; supported reasoning efforts: low, high; standard OpenAI API price unknown; do not treat as low cost."
    );
}

#[test]
fn model_and_effort_choices_are_validated_against_the_candidate_catalog() {
    let models = vec![candidate("gpt-6-sol")];

    assert_eq!(
        validate_model_choice("gpt-6-sol", &models).expect("known model"),
        &models[0]
    );
    assert!(matches!(
        validate_model_choice("not-in-picker", &models),
        Err(AutoRouteError::UnknownModel(slug)) if slug == "not-in-picker"
    ));
    validate_effort_choice(&models[0], "high").expect("supported effort");
    assert!(matches!(
        validate_effort_choice(&models[0], "ultra"),
        Err(AutoRouteError::UnsupportedEffort { model, effort })
            if model == "gpt-6-sol" && effort == "ultra"
    ));
}

#[test]
fn response_requires_the_named_choice_answer() {
    let response: DecisionResponse = serde_json::from_value(json!({
        "answers": {"model": {"choice": "gpt-6-sol", "type": "choice"}}
    }))
    .expect("decision response");

    assert_eq!(
        parse_choice_answer(&response, "model").expect("model choice"),
        "gpt-6-sol"
    );
    assert!(matches!(
        parse_choice_answer(&response, "effort"),
        Err(AutoRouteError::MissingChoice("effort"))
    ));
}

#[test]
fn routing_inputs_are_bounded_and_jev_model_has_a_default() {
    assert_eq!(
        jev_model_from_configured_value(None),
        DEFAULT_JEV_MODEL.to_string()
    );
    assert_eq!(
        jev_model_from_configured_value(Some("custom/jev")),
        "custom/jev".to_string()
    );
    assert_eq!(truncate_chars("abcdef", 3), "ab…");

    let attachments = [];
    let candidates = vec![candidate("gpt-6-sol")];
    let long_task = "x".repeat(MAX_TASK_CHARS + 5);
    let long_history = "h".repeat(MAX_HISTORY_CHARS + 5);
    let routing = routing_request(
        &long_task,
        Some(&long_history),
        &attachments,
        &candidates,
        RoutingMode::CostEffective,
    );
    let request = build_model_request("jev", &routing).expect("bounded request");

    assert_eq!(
        request["state"]["task"].as_str().expect("task").chars().count(),
        MAX_TASK_CHARS
    );
    assert_eq!(
        request["state"]["recent_user_reply_history"]
            .as_str()
            .expect("history")
            .chars()
            .count(),
        MAX_HISTORY_CHARS
    );
}
