//! TUI preference picker for routing each new task through Jev.

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionDescriptionLayout;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::picker_hint_line_for_keymap;
use codex_config::types::AutoRouteMode;

impl ChatWidget {
    pub(crate) fn show_auto_route_picker(&mut self) {
        let items = [
            (
                AutoRouteMode::Off,
                "Off",
                "Use the current model for every task.",
            ),
            (
                AutoRouteMode::CostEffective,
                "Cost-effective",
                "Prefer the least expensive model that fits the task.",
            ),
            (
                AutoRouteMode::StrongestFit,
                "Strongest fit",
                "Prefer the strongest model that fits the task.",
            ),
        ]
        .into_iter()
        .map(|(mode, name, description)| SelectionItem {
            name: name.into(),
            description: Some(description.into()),
            is_current: mode == self.local_settings.tui.auto_route,
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::AutoRouteModeSelected { mode });
            })],
            dismiss_on_select: true,
            require_explicit_confirmation: true,
            ..Default::default()
        })
        .collect();
        self.show_selection_view(SelectionViewParams {
            title: Some("AutoRoute".into()),
            description_layout: SelectionDescriptionLayout::Columns,
            footer_note: Some("Jev selects a model before each new task.".into()),
            footer_hint: Some(picker_hint_line_for_keymap(&self.bottom_pane.list_keymap())),
            items,
            ..SelectionViewParams::picker()
        });
    }
}
