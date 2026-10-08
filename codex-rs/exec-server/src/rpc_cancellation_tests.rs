use std::time::Duration;

use codex_exec_server_protocol::JSONRPCMessage;
use codex_exec_server_protocol::JSONRPCNotification;
use serde::Serialize;
use serde::Serializer;
use serde::ser::Error as _;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::timeout;

use super::MAX_IN_FLIGHT_REGULAR_CALLS;
use super::RpcCallError;
use super::RpcClient;
use super::tests::read_jsonrpc_line;
use crate::connection::JsonRpcConnection;
use crate::connection::JsonRpcTransport;

#[tokio::test]
async fn dropped_response_waiter_removes_pending_registration_without_transport_activity() {
    let (client_stdin, server_reader) = tokio::io::duplex(4096);
    let (server_writer, client_stdout) = tokio::io::duplex(4096);
    let connection =
        JsonRpcConnection::from_stdio(client_stdout, client_stdin, "test-rpc".to_string());
    let (client, _events_rx) = RpcClient::new(connection);
    let params = serde_json::json!({});
    let (request_seen_tx, request_seen_rx) = oneshot::channel();
    let (release_server_tx, release_server_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let request = read_jsonrpc_line(&mut lines).await;
        request_seen_tx
            .send(request)
            .expect("test should receive the outbound request");
        let _ = release_server_rx.await;
        drop(server_writer);
    });

    let mut call = Box::pin(client.call::<_, Value>("waiting", &params));
    assert!(futures::poll!(call.as_mut()).is_pending());
    let request = timeout(Duration::from_secs(5), request_seen_rx)
        .await
        .expect("server should read the request")
        .expect("server request notification should arrive");
    assert!(matches!(request, JSONRPCMessage::Request(_)));
    assert_eq!(client.pending_request_count().await, 1);

    drop(call);
    assert_eq!(client.pending_request_count().await, 0);

    release_server_tx
        .send(())
        .expect("server should remain connected until after cancellation is checked");
    timeout(Duration::from_secs(5), server)
        .await
        .expect("server task should finish")
        .expect("server task should not panic");
}

#[tokio::test]
async fn dropped_call_blocked_on_outbound_queue_cleans_registration_and_releases_slot() {
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(/*buffer*/ 1);
    outgoing_tx
        .send(JSONRPCMessage::Notification(JSONRPCNotification {
            method: "blocker".to_string(),
            params: None,
        }))
        .await
        .expect("outbound queue should accept the blocker");
    let (_incoming_tx, incoming_rx) = mpsc::channel(/*buffer*/ 1);
    let (_disconnected_tx, disconnected_rx) = watch::channel(/*init*/ false);
    let connection = JsonRpcConnection {
        outgoing_tx,
        incoming_rx,
        disconnected_rx,
        task_handles: Vec::new(),
        transport: JsonRpcTransport::Plain,
    };
    let (client, _events_rx) = RpcClient::new(connection);
    let params = serde_json::json!({});

    let mut call = Box::pin(client.call::<_, Value>("blocked", &params));
    assert!(futures::poll!(call.as_mut()).is_pending());
    assert_eq!(client.pending_request_count().await, 1);
    assert_eq!(
        client.shared_call_slots.available_permits(),
        MAX_IN_FLIGHT_REGULAR_CALLS - 1
    );

    drop(call);
    assert_eq!(client.pending_request_count().await, 0);
    assert_eq!(
        client.shared_call_slots.available_permits(),
        MAX_IN_FLIGHT_REGULAR_CALLS
    );

    let mut replacement = Box::pin(client.call::<_, Value>("replacement", &params));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    assert_eq!(client.pending_request_count().await, 1);
    assert_eq!(
        client.shared_call_slots.available_permits(),
        MAX_IN_FLIGHT_REGULAR_CALLS - 1
    );
    drop(replacement);
    assert_eq!(client.pending_request_count().await, 0);
    assert_eq!(
        client.shared_call_slots.available_permits(),
        MAX_IN_FLIGHT_REGULAR_CALLS
    );
    assert!(
        outgoing_rx.try_recv().is_ok(),
        "blocker should remain queued"
    );
}

struct FailingParams;

impl Serialize for FailingParams {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(S::Error::custom("intentional params serialization failure"))
    }
}

#[tokio::test]
async fn params_serialization_error_removes_pending_registration_without_sending() {
    let (outgoing_tx, mut outgoing_rx) = mpsc::channel(/*buffer*/ 1);
    let (_incoming_tx, incoming_rx) = mpsc::channel(/*buffer*/ 1);
    let (_disconnected_tx, disconnected_rx) = watch::channel(/*init*/ false);
    let connection = JsonRpcConnection {
        outgoing_tx,
        incoming_rx,
        disconnected_rx,
        task_handles: Vec::new(),
        transport: JsonRpcTransport::Plain,
    };
    let (client, _events_rx) = RpcClient::new(connection);

    let result = timeout(
        Duration::from_secs(5),
        client.call::<_, Value>("unserializable", &FailingParams),
    )
    .await
    .expect("serialization failure should return promptly");
    let Err(RpcCallError::Json(error)) = result else {
        panic!("expected parameters to fail JSON serialization");
    };
    assert_eq!(
        error.to_string(),
        "intentional params serialization failure"
    );
    assert_eq!(client.pending_request_count().await, 0);
    assert!(
        outgoing_rx.try_recv().is_err(),
        "serialization should fail before send"
    );
}
