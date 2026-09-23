use super::helpers::render_bottom_popup;
use super::*;
use crate::chatwidget::autoroute::PendingAutoRoute;
use codex_config::types::AutoRouteMode;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use crossterm::event::KeyCode;
use pretty_assertions::assert_eq;

async fn ready_for_confirmation() -> (
    ChatWidget,
    tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    uuid::Uuid,
    String,
    String,
) {
    let (mut chat, mut events, _ops) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_auto_route_mode(AutoRouteMode::CostEffective);
    while events.try_recv().is_ok() {}
    let id = uuid::Uuid::new_v4();
    let preset = chat
        .model_catalog()
        .try_list_models()
        .expect("model catalog")
        .into_iter()
        .find(|preset| preset.model == chat.current_model())
        .expect("current model preset");
    let effort = preset
        .supported_reasoning_efforts
        .first()
        .map(|effort| effort.effort.as_str().to_string())
        .unwrap_or_else(|| preset.default_reasoning_effort.as_str().to_string());
    let model = preset.model;
    chat.pending_auto_route = Some(PendingAutoRoute {
        request_id: id,
        user_message: UserMessage::from("held task"),
        history_record: UserMessageHistoryRecord::UserMessageText,
        source: UserMessageSource::Prompt,
        shell_escape_policy: ShellEscapePolicy::Allow,
        mode: AutoRouteMode::CostEffective,
        recommendation: None,
        failure: None,
    });
    chat.on_auto_route_resolved(
        id,
        Ok(crate::autoroute_client::RouteRecommendation {
            model: model.clone(),
            effort: effort.clone(),
        }),
    );
    (chat, events, id, model, effort)
}

#[tokio::test]
async fn auto_route_confirmation_picker_shows_jev_choice_and_cancel_restores_task() {
    let (mut chat, mut events, id, _model, _effort) = ready_for_confirmation().await;
    insta::assert_snapshot!(render_bottom_popup(&chat, /*width*/ 80));

    chat.handle_key_event(KeyCode::Esc.into());
    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::AutoRouteCancelled { request_id, .. }) if request_id == id
    ));
    chat.cancel_auto_route(id);

    assert_eq!(chat.pending_auto_route, None);
    assert_eq!(chat.bottom_pane.composer_text(), "held task");
}

#[tokio::test]
async fn auto_route_confirmation_submits_only_after_selection() {
    let (mut chat, mut events, id, model, effort) = ready_for_confirmation().await;

    chat.handle_key_event(KeyCode::Enter.into());
    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::AutoRouteConfirmed {
            request_id,
            model: selected_model,
            effort: selected_effort,
            ..
        }) if request_id == id
            && selected_model == model
            && selected_effort == effort.parse::<ReasoningEffortConfig>().unwrap()
    ));
    assert!(chat.pending_auto_route.is_some());
}

#[tokio::test]
async fn auto_route_failure_offers_current_model_and_effort_for_confirmation() {
    let (mut chat, mut events, id, model, effort) = ready_for_confirmation().await;
    chat.on_auto_route_resolved(id, Err("OPENROUTER_API_KEY is not set".to_string()));

    let picker = render_bottom_popup(&chat, /*width*/ 100);
    assert!(picker.contains("Jev unavailable · current model and effort"));
    assert!(picker.contains("Jev could not route this task (OPENROUTER_API_KEY is not set)"));

    chat.handle_key_event(KeyCode::Enter.into());
    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::AutoRouteConfirmed {
            request_id,
            model: selected_model,
            effort: selected_effort,
            ..
        }) if request_id == id
            && selected_model == model
            && selected_effort == effort.parse::<ReasoningEffortConfig>().unwrap()
    ));
}

#[tokio::test]
async fn pending_route_is_restored_when_thread_input_state_is_swapped() {
    let (mut chat, _events, id, _model, _effort) = ready_for_confirmation().await;
    let state = chat
        .capture_thread_input_state()
        .expect("thread input state");
    chat.pending_auto_route = None;

    chat.restore_thread_input_state(
        Some(state),
        ThreadInputStateRestoreMode {
            preserve_in_flight_turn: false,
        },
    );

    assert_eq!(
        chat.pending_auto_route
            .as_ref()
            .map(|pending| pending.request_id),
        Some(id)
    );
    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Confirm model for this turn"));
}
