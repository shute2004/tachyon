use super::*;
use codex_config::types::WindowsSandboxModeToml;
use codex_utils_absolute_path::test_support::PathExt;
use codex_utils_approval_presets::builtin_approval_presets;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[tokio::test]
async fn legacy_windows_setup_and_direct_enable_are_rejected_without_writes() -> Result<()> {
    let (mut app, mut app_event_rx, mut op_rx) = make_test_app_with_channels().await;
    let codex_home = tempdir()?;
    app.config.codex_home = codex_home.path().to_path_buf().abs();
    let config_path = codex_home.path().join("config.toml").abs();
    let config_contents = "[windows]\nsandbox = \"elevated\"\n";
    std::fs::write(config_path.as_path(), config_contents)?;
    let user_config = toml::from_str::<toml::Value>(config_contents)?;
    app.config.config_layer_stack = app
        .config
        .config_layer_stack
        .with_user_config(&config_path, user_config)?;
    app.config.permissions.windows_sandbox_mode = Some(WindowsSandboxModeToml::Elevated);
    app.chat_widget
        .set_windows_sandbox_mode(Some(WindowsSandboxModeToml::Elevated));
    let mut app_server = start_config_write_test_app_server(&app).await?;
    let preset = builtin_approval_presets()
        .into_iter()
        .find(|preset| preset.id == "auto")
        .expect("auto approval preset");
    let events = [
        AppEvent::BeginWindowsSandboxLegacySetup {
            preset: preset.clone(),
            profile_selection: None,
        },
        AppEvent::EnableWindowsSandboxForAgentMode {
            preset,
            mode: crate::app_event::WindowsSandboxEnableMode::Legacy,
            profile_selection: None,
        },
    ];
    let mut tui = crate::tui::test_support::make_test_tui()?;

    for event in events {
        while app_event_rx.try_recv().is_ok() {}
        while op_rx.try_recv().is_ok() {}
        let config_before = std::fs::read(config_path.as_path())?;
        let app_mode_before = app.config.permissions.windows_sandbox_mode;
        let widget_mode_before = app
            .chat_widget
            .config_ref()
            .permissions
            .windows_sandbox_mode;

        app.chat_widget.show_windows_sandbox_setup_status();
        let control = app.handle_event(&mut tui, &mut app_server, event).await?;

        assert!(matches!(control, AppRunControl::Continue));
        assert_eq!(std::fs::read(config_path.as_path())?, config_before);
        assert_eq!(app.config.permissions.windows_sandbox_mode, app_mode_before);
        assert_eq!(
            app.chat_widget
                .config_ref()
                .permissions
                .windows_sandbox_mode,
            widget_mode_before
        );

        let event = app_event_rx
            .try_recv()
            .expect("rejection should queue its error history cell");
        let AppEvent::InsertHistoryCell(cell) = event else {
            panic!("expected only an error history cell, got {event:?}");
        };
        insta::assert_snapshot!(
            lines_to_single_string(&cell.display_lines(/*width*/ 120)),
            @"■ Legacy Windows sandbox is disabled; set up the elevated sandbox instead."
        );
        assert!(
            app_event_rx.try_recv().is_err(),
            "rejection must not queue setup, enable, or preset-changing events"
        );
        assert!(
            op_rx.try_recv().is_err(),
            "rejection must not queue a Codex operation"
        );
    }

    app_server.shutdown().await?;
    Ok(())
}
