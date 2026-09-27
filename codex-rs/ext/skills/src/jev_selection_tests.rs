use std::sync::Arc;
use std::sync::Mutex;

use codex_config::DecisionModelsConfig;
use codex_config::DecisionModelsProvider;
use codex_config::types::RedactedString;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_http_client::RouteAwareClientPool;
use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;
use crate::catalog::SkillAuthority;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;

fn entry(name: &str, id: &str) -> SkillCatalogEntry {
    SkillCatalogEntry::new(
        SkillPackageId(id.to_string()),
        SkillAuthority::new(SkillSourceKind::Host, "host"),
        name,
        format!("Instructions for {name}."),
        SkillResourceId::new(format!("{id}/SKILL.md")),
    )
    .with_display_path(format!("/skills/{id}/SKILL.md"))
}

fn candidates_for(entries: &[SkillCatalogEntry]) -> Vec<SkillCandidate> {
    eligible_candidates(
        &SkillSuggestionContext {
            request: "Review a workbook".to_string(),
            recent_requests: Vec::new(),
        },
        entries,
    )
}

fn response(answers: Value) -> DecisionResponse {
    serde_json::from_value(json!({"answers": answers})).expect("valid fixture response")
}

fn selector(endpoint: String) -> JevSkillSelector {
    JevSkillSelector::with_endpoint(
        RouteAwareClientPool::new(
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            ClientRouteClass::Api,
        ),
        endpoint,
    )
}

#[test]
fn filters_disabled_skills_and_applies_ranking_confidence_and_distinct_name_limits() {
    let mut disabled = entry("disabled", "disabled");
    disabled.enabled = false;
    let mut hidden = entry("hidden", "hidden");
    hidden.prompt_visible = false;
    let entries = [
        entry("PDF", "pdf"),
        entry("PDF", "pdf-copy"),
        entry("Spreadsheets", "sheets"),
        entry("Charts", "charts"),
        entry("Memo", "memo"),
        disabled,
        hidden,
    ];
    let candidates = candidates_for(&entries);
    assert_eq!(candidates.len(), 5);

    let ranked = parse_ranking(
        &response(json!({"rank": {
            "type": "choice", "choice": "skill_1",
            "probabilities": {
                "skill_0": 0.2, "skill_1": 0.6, "skill_2": 0.1,
                "skill_3": 0.06, "skill_4": 0.04
            }
        }})),
        &candidates,
        false,
    )
    .expect("valid ranking");
    assert_eq!(ranked[0].key, "skill_1");

    let fits = response(json!({
        "fit_skill_0": {"type": "noul", "noul": 0.91},
        "fit_skill_1": {"type": "noul", "noul": 0.90},
        "fit_skill_2": {"type": "noul", "noul": 0.85},
        "fit_skill_3": {"type": "noul", "noul": 0.849},
        "fit_skill_4": {"type": "noul", "noul": 0.80}
    }));
    let suggestions = parse_verification(&fits, &ranked).expect("valid fit answers");
    assert_eq!(
        suggestions
            .iter()
            .map(|suggestion| (suggestion.name.as_str(), suggestion.fit_probability))
            .collect::<Vec<_>>(),
        [("PDF", 0.91), ("Spreadsheets", 0.85)]
    );

    let many_entries = (0..5)
        .map(|index| entry(&format!("Skill {index}"), &format!("skill-{index}")))
        .collect::<Vec<_>>();
    let many_candidates = candidates_for(&many_entries);
    let many_fits = (0..5)
        .map(|index| {
            (
                format!("fit_skill_{index}"),
                json!({"type": "noul", "noul": 0.9}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    assert_eq!(
        parse_verification(&response(Value::Object(many_fits)), &many_candidates)
            .expect("valid fits")
            .len(),
        MAX_SUGGESTIONS
    );
    let no_fits = (0..5)
        .map(|index| {
            (
                format!("fit_skill_{index}"),
                json!({"type": "noul", "noul": 0.84}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    assert!(
        parse_verification(&response(Value::Object(no_fits)), &many_candidates)
            .expect("valid no-match")
            .is_empty()
    );
}

#[test]
fn rejects_malformed_answers_and_bypasses_short_or_explicit_inputs() {
    assert!(
        parse_ranking(
            &response(json!({})),
            &candidates_for(&[entry("PDF", "pdf")]),
            false
        )
        .is_err()
    );
    let text = |text: &str| UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    };
    let pdf = entry("pdf:pdf", "pdf");
    let entries = [pdf];
    assert!(has_explicit_skill_selection(
        &[UserInput::Skill {
            name: "PDF".to_string(),
            path: "/skills/pdf/SKILL.md".into(),
        }],
        &entries,
    ));
    assert!(has_explicit_skill_selection(
        &[UserInput::Mention {
            name: "PDF".to_string(),
            path: "skill://pdf/SKILL.md".to_string(),
        }],
        &entries,
    ));
    assert!(has_explicit_skill_selection(
        &[text("Use $pdf for the task")],
        &entries,
    ));
    assert!(!has_explicit_skill_selection(
        &[text("Use $browser for the task")],
        &entries,
    ));
    assert_eq!(substantive_request(&[text("continue")]), None);
    assert_eq!(
        substantive_request(&[text("Review the workbook formulas")]),
        Some("Review the workbook formulas".to_string())
    );
}

#[test]
fn local_error_hints_are_sanitized_and_bounded() {
    let control_character = JevSkillSelectionError::OllayaModelNotFound("bad\nmodel".into());
    assert_eq!(
        control_character.user_message(),
        "Ollaya model `badmodel` is not installed; run `ollaya pull badmodel`"
    );

    let long_model_name = JevSkillSelectionError::OllayaModelNotFound("m".repeat(512));
    let message = long_model_name.user_message();
    assert_eq!(message.chars().count(), MAX_ERROR_MESSAGE_CHARS);
    assert!(message.ends_with('…'));
}

#[test]
fn follow_up_history_keeps_two_prior_requests_and_bounds_each_request() {
    let history = SkillSuggestionHistory::default();
    history.context_for_turn("1", "Summarize a PDF");
    history.context_for_turn("2", "Review workbook formulas");
    let context = history.context_for_turn("3", &"x".repeat(MAX_TASK_CHARS + 100));

    assert_eq!(
        context.recent_requests,
        ["Summarize a PDF", "Review workbook formulas"]
    );
    assert_eq!(context.request.chars().count(), MAX_TASK_CHARS + 1);
    assert!(context.request.ends_with('…'));
}

#[tokio::test]
async fn selector_sends_bounded_metadata_in_two_passes_and_keeps_only_confident_fits() {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer test-key"))
        .and(path("/decisions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("json request");
            observed.lock().expect("requests lock").push(body.clone());
            let answers = if body["questions"].get("rank").is_some() {
                json!({"rank": {
                    "type": "choice", "choice": "skill_0",
                    "probabilities": {"skill_0": 0.7, "skill_1": 0.3}
                }})
            } else {
                json!({
                    "fit_skill_0": {"type": "noul", "noul": 0.85},
                    "fit_skill_1": {"type": "noul", "noul": 0.849}
                })
            };
            ResponseTemplate::new(200).set_body_json(json!({"answers": answers}))
        })
        .expect(2)
        .mount(&server)
        .await;

    let context = SkillSuggestionContext {
        request: "Review workbook formulas".to_string(),
        recent_requests: vec!["Create a workbook".to_string()],
    };
    let suggestions = selector(format!("{}/decisions", server.uri()))
        .suggest(
            &DecisionModelsConfig {
                openrouter_api_key: Some(RedactedString::from("test-key")),
                ..Default::default()
            },
            None,
            &context,
            &[entry("spreadsheets", "sheets"), entry("pdf", "pdf")],
        )
        .await
        .expect("Jev result");

    assert_eq!(suggestions[0].name, "spreadsheets");
    assert_eq!(suggestions[0].fit_probability, 0.85);
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "~typesafe/jev-latest");
    assert_eq!(
        requests[0]["state"]["recent_requests"][0],
        "Create a workbook"
    );
    assert_eq!(requests[0]["state"]["skills"][0]["source"], "host");
    assert!(requests.iter().all(|request| {
        !request.to_string().contains("SKILL.md") && !request.to_string().contains("test-key")
    }));
}

#[tokio::test]
async fn ollaya_ranking_has_a_no_match_choice_for_a_single_candidate() {
    let server = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(move |request: &Request| {
            assert!(request.headers.get("authorization").is_none());
            let body: Value = serde_json::from_slice(&request.body).expect("json request");
            observed.lock().expect("requests lock").push(body);
            ResponseTemplate::new(200).set_body_json(json!({
                "answers": {
                    "rank": {"type": "choice", "choice": "no_match"}
                }
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    let selector = JevSkillSelector::new(RouteAwareClientPool::new(
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        ClientRouteClass::Api,
    ));
    let config = DecisionModelsConfig {
        provider: DecisionModelsProvider::Ollaya,
        ollaya_base_url: Some(server.uri()),
        ..Default::default()
    };
    let suggestions = selector
        .suggest(
            &config,
            None,
            &SkillSuggestionContext {
                request: "Review this request".to_string(),
                recent_requests: Vec::new(),
            },
            &[entry("PDF", "pdf")],
        )
        .await
        .expect("Ollaya no-match response");

    assert!(suggestions.is_empty());
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], "decider:2b");
    assert_eq!(
        requests[0]["questions"]["rank"]["criteria"]["no_match"],
        "No listed skill directly applies to the user's request."
    );
    assert_eq!(
        requests[0]["questions"]["rank"]["criteria"]["skill_0"],
        "PDF; source: host; description: Instructions for PDF."
    );
}

#[tokio::test]
async fn missing_key_malformed_answer_and_network_failure_are_unavailable_errors() {
    let entries = [entry("PDF", "pdf")];
    let context = SkillSuggestionContext {
        request: "Summarize a PDF".to_string(),
        recent_requests: Vec::new(),
    };
    assert!(matches!(
        selector("http://127.0.0.1:1/decisions".to_string())
            .suggest(&DecisionModelsConfig::default(), None, &context, &entries)
            .await,
        Err(JevSkillSelectionError::MissingApiKey)
    ));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/decisions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(matches!(
        selector(format!("{}/decisions", server.uri()))
            .suggest(
                &DecisionModelsConfig {
                    openrouter_api_key: Some(RedactedString::from("test-key")),
                    ..Default::default()
                },
                None,
                &context,
                &entries,
            )
            .await,
        Err(JevSkillSelectionError::InvalidResponse(_))
    ));
    assert!(matches!(
        selector("http://127.0.0.1:1/decisions".to_string())
            .suggest(
                &DecisionModelsConfig {
                    openrouter_api_key: Some(RedactedString::from("test-key")),
                    ..Default::default()
                },
                None,
                &context,
                &entries,
            )
            .await,
        Err(JevSkillSelectionError::Request(_))
    ));
}
