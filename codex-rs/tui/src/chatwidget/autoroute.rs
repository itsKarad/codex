//! Hold new turns for Jev's model and reasoning-effort recommendation.

use super::*;
use crate::autoroute_client::AttachmentMetadata;
use crate::autoroute_client::ModelCandidate;
use crate::autoroute_client::RouteRecommendation;
use crate::bottom_pane::SelectionDescriptionLayout;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use codex_config::types::AutoRouteMode;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingAutoRoute {
    pub(super) request_id: uuid::Uuid,
    pub(super) user_message: UserMessage,
    pub(super) history_record: UserMessageHistoryRecord,
    pub(super) source: UserMessageSource,
    pub(super) shell_escape_policy: ShellEscapePolicy,
    pub(super) mode: AutoRouteMode,
    pub(super) recommendation: Option<RouteRecommendation>,
    pub(super) failure: Option<String>,
}

pub(super) struct AutoRouteTurnOverride {
    pub(super) model: String,
    pub(super) effort: ReasoningEffortConfig,
}

impl ChatWidget {
    pub(super) fn should_auto_route_submission(
        &self,
        user_message: &UserMessage,
        shell_escape_policy: ShellEscapePolicy,
    ) -> bool {
        self.local_settings.tui.auto_route != AutoRouteMode::Off
            && !self.turn_lifecycle.agent_turn_running
            && !(shell_escape_policy == ShellEscapePolicy::Allow
                && user_message.text.starts_with('!'))
    }

    pub(super) fn begin_auto_route(
        &mut self,
        user_message: UserMessage,
        history_record: UserMessageHistoryRecord,
        shell_escape_policy: ShellEscapePolicy,
        source: UserMessageSource,
    ) {
        let mode = self.local_settings.tui.auto_route;
        let pending = PendingAutoRoute {
            request_id: uuid::Uuid::new_v4(),
            user_message,
            history_record,
            source,
            shell_escape_policy,
            mode,
            recommendation: None,
            failure: None,
        };
        self.pending_auto_route = Some(pending);
        self.send_auto_route_request();
        self.add_info_message(
            "AutoRoute is asking Jev to choose a model for this turn.".to_string(),
            /*hint*/ None,
        );
        self.refresh_pending_input_preview();
        self.request_redraw();
    }

    pub(super) fn send_auto_route_request(&mut self) {
        let Some(previous_request_id) = self
            .pending_auto_route
            .as_ref()
            .map(|pending| pending.request_id)
        else {
            return;
        };
        if self.local_settings.tui.auto_route == AutoRouteMode::Off {
            self.cancel_auto_route(previous_request_id);
            return;
        }
        let request_id = uuid::Uuid::new_v4();
        let Some(pending) = self.pending_auto_route.as_mut() else {
            return;
        };
        pending.request_id = request_id;
        pending.recommendation = None;
        pending.failure = None;
        let request_id = pending.request_id;
        let thread_id = self.thread_id;
        let (mode, task, attachments) = (
            pending.mode,
            pending.user_message.text.clone(),
            attachment_metadata(&pending.user_message),
        );
        let candidates = self.auto_route_candidates();
        self.app_event_tx.send(AppEvent::AutoRouteRequested {
            request_id,
            thread_id,
            mode,
            task,
            attachments,
            candidates,
        });
    }

    pub(crate) fn on_auto_route_resolved(
        &mut self,
        request_id: uuid::Uuid,
        result: Result<RouteRecommendation, String>,
    ) {
        if !self.is_auto_route_pending(request_id) {
            return;
        }
        if self.local_settings.tui.auto_route == AutoRouteMode::Off {
            self.cancel_auto_route(request_id);
            return;
        }
        let fallback = result.is_err().then(|| self.current_route_recommendation());
        let Some(pending) = self
            .pending_auto_route
            .as_mut()
            .filter(|pending| pending.request_id == request_id)
        else {
            return;
        };
        match result {
            Ok(recommendation) => {
                pending.recommendation = Some(recommendation);
                pending.failure = None;
            }
            Err(error) => {
                pending.recommendation = fallback;
                pending.failure = Some(error);
            }
        }
        self.show_auto_route_confirmation(request_id);
    }

    pub(crate) fn cancel_auto_route(&mut self, request_id: uuid::Uuid) {
        let Some(pending) = self
            .pending_auto_route
            .take_if(|pending| pending.request_id == request_id)
        else {
            return;
        };
        self.restore_user_message_to_composer(pending.user_message);
        self.refresh_pending_input_preview();
        self.request_redraw();
    }

    pub(crate) fn is_auto_route_pending(&self, request_id: uuid::Uuid) -> bool {
        self.pending_auto_route
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id)
    }

    pub(crate) fn submit_confirmed_auto_route(
        &mut self,
        request_id: uuid::Uuid,
        model: String,
        effort: ReasoningEffortConfig,
    ) {
        let Some(pending) = self
            .pending_auto_route
            .take_if(|pending| pending.request_id == request_id)
        else {
            return;
        };
        self.submit_auto_routed_user_message(
            pending.user_message,
            pending.history_record,
            pending.shell_escape_policy,
            pending.source,
            model,
            effort,
        );
    }

    pub(crate) fn restore_pending_auto_route(&mut self) {
        let Some((request_id, has_recommendation)) = self
            .pending_auto_route
            .as_ref()
            .map(|pending| (pending.request_id, pending.recommendation.is_some()))
        else {
            return;
        };
        if self.local_settings.tui.auto_route == AutoRouteMode::Off {
            self.cancel_auto_route(request_id);
        } else if has_recommendation {
            self.show_auto_route_confirmation(request_id);
        } else {
            self.send_auto_route_request();
        }
    }

    fn show_auto_route_confirmation(&mut self, request_id: uuid::Uuid) {
        let Some(pending) = self
            .pending_auto_route
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
        else {
            return;
        };
        let Some(recommendation) = pending.recommendation.as_ref() else {
            return;
        };

        let presets = self.model_catalog.try_list_models().unwrap_or_default();
        let mut candidates: Vec<ModelCandidate> = presets
            .iter()
            .filter(|preset| preset.show_in_picker)
            .map(model_candidate)
            .collect();
        let selected = candidates
            .iter()
            .find(|candidate| candidate.slug == recommendation.model)
            .cloned()
            .or_else(|| {
                presets
                    .iter()
                    .find(|preset| preset.model == recommendation.model)
                    .map(model_candidate)
            })
            .unwrap_or_else(|| self.current_model_candidate());
        if !candidates
            .iter()
            .any(|candidate| candidate.slug == selected.slug)
        {
            candidates.insert(0, selected.clone());
        }

        let mut choices = vec![(selected, recommendation.effort.clone(), true)];
        for candidate in candidates {
            for effort in &candidate.supported_efforts {
                if candidate.slug != recommendation.model || *effort != recommendation.effort {
                    choices.push((candidate.clone(), effort.clone(), false));
                }
            }
        }

        let mut items = choices
            .into_iter()
            .filter_map(|(candidate, effort, is_recommended)| {
                let model = candidate.slug.clone();
                let selected_effort = effort.parse::<ReasoningEffortConfig>().ok()?;
                let event_thread_id = self.thread_id;
                let display_name = candidate.name;
                Some(SelectionItem {
                    name: format!("{display_name} · {effort}"),
                    description: is_recommended.then(|| {
                        if pending.failure.is_some() {
                            "Jev unavailable · current model and effort".to_string()
                        } else {
                            "Jev's recommendation".to_string()
                        }
                    }),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::AutoRouteConfirmed {
                            request_id,
                            thread_id: event_thread_id,
                            model: model.clone(),
                            effort: selected_effort.clone(),
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        let cancel_thread_id = self.thread_id;
        items.push(SelectionItem {
            name: "Cancel and restore task".to_string(),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::AutoRouteCancelled {
                    request_id,
                    thread_id: cancel_thread_id,
                });
            })],
            dismiss_on_select: true,
            ..Default::default()
        });
        let error_subtitle = pending.failure.as_ref().map(|error| {
            format!(
                "Jev could not route this task ({}) · confirm the current model or choose another",
                truncate_error(error)
            )
        });
        let cancel_thread_id = self.thread_id;
        self.show_selection_view(SelectionViewParams {
            title: Some("Confirm model for this turn".to_string()),
            subtitle: error_subtitle,
            description_layout: SelectionDescriptionLayout::Columns,
            footer_note: Some("Selecting a model submits the held task.".into()),
            on_cancel: Some(Box::new(move |tx| {
                tx.send(AppEvent::AutoRouteCancelled {
                    request_id,
                    thread_id: cancel_thread_id,
                });
            })),
            initial_selected_idx: Some(0),
            items,
            ..SelectionViewParams::picker()
        });
    }

    fn auto_route_candidates(&self) -> Vec<ModelCandidate> {
        self.model_catalog
            .try_list_models()
            .unwrap_or_default()
            .into_iter()
            .filter(|preset| preset.show_in_picker)
            .map(|preset| model_candidate(&preset))
            .collect()
    }

    fn current_model_candidate(&self) -> ModelCandidate {
        self.model_catalog
            .try_list_models()
            .unwrap_or_default()
            .into_iter()
            .find(|preset| preset.model == self.current_model())
            .map(|preset| model_candidate(&preset))
            .unwrap_or_else(|| ModelCandidate {
                slug: self.current_model().to_string(),
                name: self.model_display_name().to_string(),
                description: String::new(),
                input_modalities: Vec::new(),
                supported_efforts: vec![
                    self.current_reasoning_effort()
                        .unwrap_or_default()
                        .as_str()
                        .to_string(),
                ],
            })
    }

    fn current_route_recommendation(&self) -> RouteRecommendation {
        let candidate = self.current_model_candidate();
        let default_effort = self
            .model_catalog
            .try_list_models()
            .unwrap_or_default()
            .into_iter()
            .find(|preset| preset.model == candidate.slug)
            .map(|preset| preset.default_reasoning_effort.as_str().to_string());
        RouteRecommendation {
            model: candidate.slug,
            effort: self
                .current_reasoning_effort()
                .map(|effort| effort.as_str().to_string())
                .or(default_effort)
                .unwrap_or_else(|| "medium".to_string()),
        }
    }
}

fn attachment_metadata(user_message: &UserMessage) -> Vec<AttachmentMetadata> {
    let image_count = user_message.local_images.len() + user_message.remote_image_urls.len();
    if image_count == 0 {
        Vec::new()
    } else {
        vec![AttachmentMetadata {
            kind: "image".to_string(),
            count: image_count,
        }]
    }
}

fn model_candidate(preset: &ModelPreset) -> ModelCandidate {
    let mut supported_efforts = preset
        .supported_reasoning_efforts
        .iter()
        .map(|option| option.effort.as_str().to_string())
        .collect::<Vec<_>>();
    if supported_efforts.is_empty() {
        supported_efforts.push(preset.default_reasoning_effort.as_str().to_string());
    }
    ModelCandidate {
        slug: preset.model.clone(),
        name: preset.display_name.clone(),
        description: preset.description.clone(),
        input_modalities: preset
            .input_modalities
            .iter()
            .map(ToString::to_string)
            .collect(),
        supported_efforts,
    }
}

fn truncate_error(error: &str) -> &str {
    const MAX_ERROR_CHARS: usize = 160;
    let end = error
        .char_indices()
        .nth(MAX_ERROR_CHARS)
        .map_or(error.len(), |(index, _)| index);
    &error[..end]
}
