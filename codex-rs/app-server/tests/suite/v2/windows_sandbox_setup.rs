#[cfg(not(windows))]
use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence_unchecked;
#[cfg(not(windows))]
use app_test_support::to_response;
use app_test_support::write_mock_responses_config_toml;
#[cfg(not(windows))]
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
#[cfg(not(windows))]
use codex_app_server_protocol::WindowsSandboxSetupCompletedNotification;
use codex_app_server_protocol::WindowsSandboxSetupMode;
use codex_app_server_protocol::WindowsSandboxSetupStartParams;
#[cfg(not(windows))]
use codex_app_server_protocol::WindowsSandboxSetupStartResponse;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(not(windows))]
// Exercise completion notification reporting without invoking Windows UAC setup.
#[tokio::test]
async fn windows_sandbox_setup_start_emits_failure_completion_on_unsupported_platform() -> Result<()>
{
    let responses = Vec::new();
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::new(),
        /*auto_compact_limit*/ 500_000,
        Some(false),
        "mock_provider",
        "compact prompt",
    )?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_windows_sandbox_setup_start_request(WindowsSandboxSetupStartParams {
            mode: WindowsSandboxSetupMode::Elevated,
            cwd: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let start_payload: WindowsSandboxSetupStartResponse = to_response(response)?;
    assert!(start_payload.started);

    let notification = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("windowsSandbox/setupCompleted"),
    )
    .await??;
    let payload: WindowsSandboxSetupCompletedNotification = serde_json::from_value(
        notification
            .params
            .context("missing windowsSandbox/setupCompleted params")?,
    )?;

    assert_eq!(payload.mode, WindowsSandboxSetupMode::Elevated);
    assert!(!payload.success);
    assert_eq!(
        payload.error.as_deref(),
        Some("elevated Windows sandbox setup is only supported on Windows")
    );
    Ok(())
}

#[tokio::test]
async fn windows_sandbox_setup_start_rejects_legacy_mode_without_mutating_config() -> Result<()> {
    let responses = Vec::new();
    let server = create_mock_responses_server_sequence_unchecked(responses).await;
    let codex_home = TempDir::new()?;
    write_mock_responses_config_toml(
        codex_home.path(),
        &server.uri(),
        &BTreeMap::new(),
        /*auto_compact_limit*/ 500_000,
        Some(false),
        "mock_provider",
        "compact prompt",
    )?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let config_path = codex_home.path().join("config.toml");
    let original_config = std::fs::read(&config_path)?;
    let request_id = mcp
        .send_windows_sandbox_setup_start_request(WindowsSandboxSetupStartParams {
            mode: WindowsSandboxSetupMode::Unelevated,
            cwd: None,
        })
        .await?;
    let err = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(
        err.error
            .message
            .contains("invalid Windows sandbox setup mode"),
        "{err:?}"
    );
    assert!(
        err.error
            .message
            .contains("legacy unelevated setup is disabled"),
        "{err:?}"
    );
    assert!(
        err.error.message.contains("use elevated mode instead"),
        "{err:?}"
    );
    assert_eq!(std::fs::read(config_path)?, original_config);
    Ok(())
}

#[tokio::test]
async fn windows_sandbox_setup_start_rejects_relative_cwd() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .without_auto_env()
        .build()
        .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "windowsSandbox/setupStart",
            Some(serde_json::json!({
                "mode": "unelevated",
                "cwd": "relative-root",
            })),
        )
        .await?;

    let err = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("Invalid request"));
    Ok(())
}
