use crate::app::tests::make_test_app_with_channels;
use crate::collaboration_modes;
use crate::legacy_core::config::ConfigBuilder;
use codex_config::LoaderOverrides;
use codex_config::types::AutoRouteMode;
use codex_protocol::config_types::ModeKind;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn auto_route_setting_persists_globally_and_exits_plan_mode() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let path = AbsolutePathBuf::from_absolute_path(home.path().join("config.toml"))?;
    std::fs::write(path.as_path(), "# User preferences\n")?;
    let (mut app, _events, _ops) = make_test_app_with_channels().await;
    app.local_settings.user_config_path = path.clone();

    let catalog = app.chat_widget.model_catalog();
    let plan = collaboration_modes::plan_mask(&catalog).expect("plan mode preset");
    app.chat_widget.set_collaboration_mask(plan);
    assert_eq!(
        app.chat_widget.effective_collaboration_mode().mode,
        ModeKind::Plan
    );

    app.save_auto_route_mode(AutoRouteMode::CostEffective).await;

    assert_eq!(
        app.local_settings.tui.auto_route,
        AutoRouteMode::CostEffective
    );
    assert_eq!(
        app.chat_widget.local_settings.tui.auto_route,
        AutoRouteMode::CostEffective
    );
    assert_eq!(app.config.tui_auto_route, AutoRouteMode::CostEffective);
    assert_eq!(
        app.chat_widget.effective_collaboration_mode().mode,
        ModeKind::Default
    );
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path.as_path())?)?;
    assert_eq!(
        saved["tui"]["auto_route"],
        toml::Value::String("cost-effective".into())
    );

    let reloaded = ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .loader_overrides(LoaderOverrides {
            user_config_path: Some(path),
            ignore_project_config: true,
            ..LoaderOverrides::without_managed_config_for_tests()
        })
        .build()
        .await?;
    assert_eq!(reloaded.tui_auto_route, AutoRouteMode::CostEffective);
    Ok(())
}
