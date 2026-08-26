//! Compatibility shim for Codex/Responses call sites that have not yet moved to `ModelTurnRuntime`.
//!
//! Generic retry policy lives under `model_runtime::retry`. This module remains temporarily so
//! Step B can migrate large call sites without changing retry behavior and runtime ownership at
//! once.

use crate::client::ModelClientSession;
use crate::model_runtime::retry::handle_retryable_model_stream_error;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_protocol::error::CodexErr;

const FALLBACK_TO_HTTP_WARNING: &str = "Falling back from WebSockets to HTTPS transport.";

pub(crate) use crate::model_runtime::retry::ModelStreamRequest as ResponsesStreamRequest;
pub(crate) use crate::model_runtime::retry::ModelStreamRetryState as ResponsesStreamRetryState;

/// Preserves the existing Responses retry surface while delegating reusable policy to the model
/// runtime. Provider/transport-specific policy inputs stay in this compatibility layer.
pub(crate) async fn handle_retryable_response_stream_error(
    retry_state: &mut ResponsesStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    turn_context: &TurnContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    let allow_unbounded_connection_retry = !turn_context.provider.info().is_amazon_bedrock();
    let suppress_first_retry_notification = sess.services.model_client.responses_websocket_enabled();
    handle_retryable_model_stream_error(
        retry_state,
        max_retries,
        err,
        sess,
        turn_context,
        request,
        allow_unbounded_connection_retry,
        suppress_first_retry_notification,
        || {
            client_session
                .try_switch_fallback_transport(
                    &turn_context.session_telemetry,
                    turn_context.model_info(),
                )
                .then(|| FALLBACK_TO_HTTP_WARNING.to_string())
        },
    )
    .await
}
