//! Persist and apply AutoRoute's TUI preference.

use super::App;
use crate::collaboration_modes;
use crate::config_update::format_config_error;
use crate::legacy_core::config::edit::ConfigEdit;
use crate::legacy_core::config::edit::ConfigEditsBuilder;
use codex_config::types::AutoRouteMode;
use codex_protocol::config_types::ModeKind;

impl App {
    pub(super) async fn save_auto_route_mode(&mut self, mode: AutoRouteMode) {
        let default_mask = if mode != AutoRouteMode::Off
            && self.chat_widget.effective_collaboration_mode().mode == ModeKind::Plan
        {
            let catalog = self.chat_widget.model_catalog();
            if let Some(default_mask) = collaboration_modes::default_mode_mask(&catalog) {
                Some(default_mask)
            } else {
                self.chat_widget.add_info_message(
                    "AutoRoute requires Default mode, which is unavailable right now.".to_string(),
                    /*hint*/ None,
                );
                return;
            }
        } else {
            None
        };

        if mode == AutoRouteMode::Off {
            self.local_settings.tui.auto_route = mode;
            self.chat_widget.set_auto_route_mode(mode);
            self.config.tui_auto_route = mode;
        }

        let result =
            ConfigEditsBuilder::for_config_path(self.local_settings.user_config_path.as_path())
                .with_edits([ConfigEdit::SetPath {
                    segments: vec!["tui".into(), "auto_route".into()],
                    value: toml_edit::value(mode.as_str()),
                }])
                .apply()
                .await;
        match result {
            Ok(()) => {
                if let Some(default_mask) = default_mask {
                    self.chat_widget
                        .set_collaboration_mask_from_user_action(default_mask);
                }
                self.local_settings.tui.auto_route = mode;
                self.chat_widget.set_auto_route_mode(mode);
                self.config.tui_auto_route = mode;
                let label = match mode {
                    AutoRouteMode::Off => "Off",
                    AutoRouteMode::CostEffective => "Cost-effective",
                    AutoRouteMode::StrongestFit => "Strongest fit",
                };
                self.chat_widget
                    .add_info_message(format!("AutoRoute set to {label}."), /*hint*/ None);
            }
            Err(error) => self.chat_widget.add_error_message(format!(
                "Failed to save AutoRoute setting: {}",
                format_config_error(&error),
            )),
        }
    }
}

#[cfg(test)]
#[path = "autoroute_settings_tests.rs"]
mod tests;
