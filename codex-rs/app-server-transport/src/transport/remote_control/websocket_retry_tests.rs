//! Checks sustained handshake failures and recovery through the reconnect loop.

use super::tests::TEST_INSTALLATION_ID;
use super::tests::TEST_REMOTE_CONTROL_SERVER_TOKEN;
use super::tests::enabled_desired_state_sender;
use super::tests::remote_control_auth_manager;
use super::tests::remote_control_enrollment;
use super::tests::remote_control_state_runtime;
use super::tests::remote_control_status_channel;
use super::tests::remote_control_url_for_listener;
use super::tests::test_current_enrollment;
use super::*;
use crate::transport::remote_control::protocol::normalize_remote_control_url;
use base64::Engine;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio_tungstenite::accept_hdr_async;
use tokio_util::sync::CancellationToken;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;

#[tokio::test]
async fn repeated_conflicts_without_cursor_stay_at_cap_and_recover() {
    assert_conflict_recovery(/*subscribe_cursor*/ None).await;
}

#[tokio::test]
async fn repeated_conflicts_with_cursor_stay_at_cap_and_recover() {
    assert_conflict_recovery(Some("stale-cursor")).await;
}

async fn assert_conflict_recovery(subscribe_cursor: Option<&str>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let remote_control_target =
        normalize_remote_control_url(&remote_control_url).expect("target should normalize");
    let codex_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&codex_home).await;
    let mut enrollment = remote_control_enrollment(Some(TEST_REMOTE_CONTROL_SERVER_TOKEN));
    enrollment.remote_control_target = remote_control_target.clone();
    let current_enrollment = test_current_enrollment(Some(enrollment.clone()));
    let (status_publisher, status_rx) = remote_control_status_channel();
    let (transport_event_tx, _transport_event_rx) = mpsc::channel(1);
    let shutdown_token = CancellationToken::new();
    let desired_state_tx = Arc::new(enabled_desired_state_sender());
    let mut websocket = RemoteControlWebsocket::new(
        RemoteControlWebsocketConfig {
            remote_control_url,
            remote_control_target: Some(remote_control_target),
            installation_id: TEST_INSTALLATION_ID.to_string(),
            server_name: "test-server".to_string(),
        },
        Some(state_db),
        remote_control_auth_manager(),
        RemoteControlChannels {
            transport_event_tx,
            status_publisher,
            current_enrollment: current_enrollment.clone(),
            pairing_persistence_key: watch::channel(None).0,
            desired_state_persistence_lock: Arc::new(Semaphore::new(1)),
        },
        shutdown_token.clone(),
        desired_state_tx,
    );
    // Start at the cap to exercise consecutive capped waits without spending
    // another minute reaching it. Keep the real local handshake and retry loop.
    websocket.reconnect_attempt = 9;
    websocket.state.lock().await.subscribe_cursor = subscribe_cursor.map(str::to_string);
    let connect_shutdown_token = shutdown_token.clone();
    let mut connect_tasks = tokio::task::JoinSet::new();
    connect_tasks.spawn(async move {
        let outcome = websocket
            .connect(
                &connect_shutdown_token,
                /*app_server_client_name*/ None,
            )
            .await;
        (websocket, outcome)
    });

    let expected_server_name_header =
        base64::engine::general_purpose::STANDARD.encode(&enrollment.server_name);
    let expected_host_device_kind = host_device_kind().await;
    let mut rejected_at = None;
    for attempt in 0..3 {
        let (stream, _) = timeout(Duration::from_secs(40), listener.accept())
            .await
            .expect("websocket connection should retry within the capped delay")
            .expect("listener should accept websocket connection");
        if let Some(rejected_at) = rejected_at {
            assert!(
                Instant::now().duration_since(rejected_at) >= REMOTE_CONTROL_RECONNECT_BACKOFF_CAP,
                "sustained HTTP 409 responses must not restart fast retries"
            );
        }
        rejected_at = Some(Instant::now());
        let handshake = accept_hdr_async(stream, |request: &Request, response: Response| {
            assert_eq!(request.method(), "GET");
            assert_eq!(
                request.uri().path(),
                "/backend-api/wham/remote/control/server"
            );
            let headers = request.headers();
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer Remote Control Token")
            );
            assert_eq!(
                headers
                    .get("x-codex-server-id")
                    .and_then(|value| value.to_str().ok()),
                Some(enrollment.server_id.as_str())
            );
            assert_eq!(
                headers
                    .get("x-codex-name")
                    .and_then(|value| value.to_str().ok()),
                Some(expected_server_name_header.as_str())
            );
            assert_eq!(
                headers
                    .get("x-codex-protocol-version")
                    .and_then(|value| value.to_str().ok()),
                Some(REMOTE_CONTROL_PROTOCOL_VERSION)
            );
            assert_eq!(
                headers
                    .get(REMOTE_CONTROL_INSTALLATION_ID_HEADER)
                    .and_then(|value| value.to_str().ok()),
                Some(TEST_INSTALLATION_ID)
            );
            assert_eq!(
                headers
                    .get(REMOTE_CONTROL_HOST_DEVICE_KIND_HEADER)
                    .and_then(|value| value.to_str().ok()),
                expected_host_device_kind
            );
            assert_eq!(
                headers
                    .get(REMOTE_CONTROL_SUBSCRIBE_CURSOR_HEADER)
                    .and_then(|value| value.to_str().ok()),
                subscribe_cursor
            );
            if attempt < 2 {
                Err(tungstenite::http::Response::builder()
                    .status(/*status*/ 409)
                    .body(Some("Remote app server already online".to_string()))
                    .expect("HTTP 409 response should build"))
            } else {
                Ok(response)
            }
        })
        .await;
        if attempt < 2 {
            assert!(handshake.is_err(), "HTTP 409 should reject the handshake");
            if attempt == 1 {
                assert!(
                    timeout(Duration::from_secs(1), listener.accept())
                        .await
                        .is_err(),
                    "capped reconnect should not retry immediately"
                );
                // Skip most of the next capped wait, but retain the >=30s
                // elapsed-time assertion when the following socket is accepted.
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(28)).await;
                tokio::time::resume();
            }
        } else {
            let server_websocket = handshake.expect("available ownership should allow recovery");
            let (websocket, outcome) = timeout(Duration::from_secs(5), connect_tasks.join_next())
                .await
                .expect("connect loop should complete after successful handshake")
                .expect("connect task should finish")
                .expect("connect task should join");
            let ConnectOutcome::Connected(client_websocket) = outcome else {
                panic!("successful handshake should produce a connected outcome");
            };
            assert_eq!(websocket.reconnect_attempt, 0);
            assert_eq!(current_enrollment.snapshot(), Some(enrollment.clone()));
            assert_eq!(
                websocket.state.lock().await.subscribe_cursor.as_deref(),
                subscribe_cursor
            );
            assert_eq!(
                status_rx.borrow().clone(),
                RemoteControlStatusChangedNotification {
                    status: RemoteControlConnectionStatus::Connected,
                    server_name: "test-server".to_string(),
                    installation_id: TEST_INSTALLATION_ID.to_string(),
                    environment_id: Some(enrollment.environment_id.clone()),
                }
            );
            drop(client_websocket);
            drop(server_websocket);
            shutdown_token.cancel();
            return;
        }
    }
}

#[test]
fn reconnect_backoff_stays_capped_during_sustained_failures() {
    let mut reconnect_attempt = 0;
    let mut reached_cap = false;
    for _ in 0..16 {
        if next_reconnect_delay(&mut reconnect_attempt) == REMOTE_CONTROL_RECONNECT_BACKOFF_CAP {
            reached_cap = true;
            break;
        }
    }
    assert!(
        reached_cap,
        "reconnect backoff should eventually reach its cap"
    );

    let capped_attempt = reconnect_attempt;
    for _ in 0..1000 {
        assert_eq!(
            next_reconnect_delay(&mut reconnect_attempt),
            REMOTE_CONTROL_RECONNECT_BACKOFF_CAP
        );
        assert_eq!(reconnect_attempt, capped_attempt);
    }
}
