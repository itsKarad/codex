//! Background work for Jev's model and reasoning-effort decisions.

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::autoroute_client::AttachmentMetadata;
use crate::autoroute_client::ModelCandidate;
use crate::autoroute_client::RoutingMode;
use crate::autoroute_client::RoutingRequest;
use crate::autoroute_client::jev_model_from_configured_value;
use crate::autoroute_client::recommend_route;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SortDirection;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadTurnsListParams;
use codex_app_server_protocol::ThreadTurnsListResponse;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnItemsView;
use codex_app_server_protocol::UserInput;
use codex_config::types::AutoRouteMode;
use codex_http_client::RouteAwareClientPool;
use codex_protocol::ThreadId;
use codex_protocol::models::MessagePhase;
use uuid::Uuid;

pub(super) struct AutoRouteRequest {
    pub(super) request_id: Uuid,
    pub(super) thread_id: Option<ThreadId>,
    pub(super) mode: AutoRouteMode,
    pub(super) task: String,
    pub(super) attachments: Vec<AttachmentMetadata>,
    pub(super) candidates: Vec<ModelCandidate>,
}

pub(super) fn start_request(
    events: AppEventSender,
    request_handle: AppServerRequestHandle,
    client: RouteAwareClientPool,
    request: AutoRouteRequest,
) {
    tokio::spawn(async move {
        let recent_history = if let Some(thread_id) = request.thread_id {
            read_recent_history(&request_handle, thread_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let api_key = std::env::var("OPENROUTER_API_KEY").ok();
        let jev_model =
            jev_model_from_configured_value(std::env::var("OPENROUTER_MODEL").ok().as_deref());
        let mode = match request.mode {
            AutoRouteMode::Off => {
                events.send(AppEvent::AutoRouteResolved {
                    request_id: request.request_id,
                    thread_id: request.thread_id,
                    result: Err("AutoRoute was turned off".to_string()),
                });
                return;
            }
            AutoRouteMode::CostEffective => RoutingMode::CostEffective,
            AutoRouteMode::StrongestFit => RoutingMode::StrongestFit,
        };
        let routing_request = RoutingRequest {
            mode,
            task: &request.task,
            recent_history: recent_history.as_deref(),
            attachments: &request.attachments,
            candidates: &request.candidates,
        };
        let result = recommend_route(&client, api_key.as_deref(), &jev_model, &routing_request)
            .await
            .map_err(|error| error.to_string());
        events.send(AppEvent::AutoRouteResolved {
            request_id: request.request_id,
            thread_id: request.thread_id,
            result,
        });
    });
}

async fn read_recent_history(
    request_handle: &AppServerRequestHandle,
    thread_id: ThreadId,
) -> Result<Option<String>, codex_app_server_client::TypedRequestError> {
    let turns: ThreadTurnsListResponse = request_handle
        .request_typed(ClientRequest::ThreadTurnsList {
            request_id: RequestId::String(format!("autoroute-history-{}", Uuid::new_v4())),
            params: ThreadTurnsListParams {
                thread_id: thread_id.to_string(),
                cursor: None,
                limit: Some(8),
                sort_direction: Some(SortDirection::Desc),
                items_view: Some(TurnItemsView::Summary),
            },
        })
        .await?;
    Ok(recent_user_reply_pairs(&turns.data))
}

fn recent_user_reply_pairs(turns_descending: &[Turn]) -> Option<String> {
    let mut pairs = Vec::new();
    for turn in turns_descending.iter().rev() {
        let mut user_text = String::new();
        let mut reply_text = None;
        for item in &turn.items {
            match item {
                ThreadItem::UserMessage { content, .. } => {
                    user_text = content
                        .iter()
                        .filter_map(|part| match part {
                            UserInput::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                ThreadItem::AgentMessage {
                    text,
                    phase: Some(MessagePhase::FinalAnswer),
                    ..
                } => reply_text = Some(text.as_str()),
                _ => {}
            }
        }
        if !user_text.is_empty()
            && let Some(reply_text) = reply_text
            && !reply_text.is_empty()
        {
            pairs.push(format!("User: {user_text}\nAssistant: {reply_text}"));
        }
    }
    if pairs.is_empty() {
        return None;
    }
    let mut history = pairs.into_iter().rev().take(2).collect::<Vec<_>>();
    history.reverse();
    Some(truncate_chars(&history.join("\n\n"), 4_000))
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
#[path = "autoroute_tests.rs"]
mod tests;
