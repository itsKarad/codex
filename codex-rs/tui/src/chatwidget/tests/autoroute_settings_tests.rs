use super::helpers::render_bottom_popup;
use super::*;
use codex_config::types::AutoRouteMode;
use crossterm::event::KeyCode;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn auto_route_picker_shows_modes_and_requires_confirmation() {
    let (mut chat, mut events, _ops) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.show_auto_route_picker();
    insta::assert_snapshot!(render_bottom_popup(&chat, /*width*/ 80));

    chat.handle_key_event(KeyCode::Down.into());
    chat.handle_key_event(KeyCode::Esc.into());
    assert_eq!(chat.local_settings.tui.auto_route, AutoRouteMode::Off);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, AppEvent::AutoRouteModeSelected { .. }));
    }

    chat.show_auto_route_picker();
    chat.handle_key_event(KeyCode::Down.into());
    chat.handle_key_event(KeyCode::Enter.into());
    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::AutoRouteModeSelected {
            mode: AutoRouteMode::CostEffective
        })
    ));
}

#[tokio::test]
async fn entering_plan_mode_disables_auto_route() {
    let (mut chat, mut events, _ops) = make_chatwidget_manual(/*model_override*/ None).await;
    while events.try_recv().is_ok() {}
    chat.set_auto_route_mode(AutoRouteMode::CostEffective);
    let plan_mask =
        crate::collaboration_modes::plan_mask(&chat.model_catalog()).expect("plan mode preset");

    chat.set_collaboration_mask(plan_mask);

    assert_eq!(chat.local_settings.tui.auto_route, AutoRouteMode::Off);
    assert!(matches!(
        events.try_recv(),
        Ok(AppEvent::AutoRouteModeSelected {
            mode: AutoRouteMode::Off
        })
    ));
}
