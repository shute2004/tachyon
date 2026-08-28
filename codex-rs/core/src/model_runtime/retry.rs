//! Provider-neutral model-stream retry policy used during model-runtime extraction.
//!
//! Concrete backend recovery mechanisms and backend-specific policy inputs are supplied by the
//! caller. The current Codex adapter may still map recovery to Responses WebSocket-to-HTTP
//! fallback, but that mechanism is not part of this module's contract.

use std::time::Duration;

use super::ModelTurnRuntime;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::util::backoff;
use codex_features::Feature;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use tracing::warn;

const INITIAL_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub(crate) enum ModelStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

pub(crate) struct ModelStreamRetryState {
    retries: u64,
    connection_retries: u64,
    connection_retry_delay: Duration,
}

impl Default for ModelStreamRetryState {
    fn default() -> Self {
        Self {
            retries: 0,
            connection_retries: 0,
            connection_retry_delay: INITIAL_CONNECTION_RETRY_DELAY,
        }
    }
}

/// Retry entry point for call sites that already own a `ModelTurnRuntime`.
///
/// `allow_unbounded_connection_retry` is an explicit policy input so the generic retry module does
/// not need to know which concrete backends opt out of that behavior.
pub(crate) async fn handle_retryable_turn_runtime_error(
    retry_state: &mut ModelStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    turn_runtime: &mut ModelTurnRuntime,
    sess: &Session,
    turn_context: &TurnContext,
    request: ModelStreamRequest,
    allow_unbounded_connection_retry: bool,
) -> Result<(), CodexErr> {
    let suppress_first_retry_notification = turn_runtime.suppress_first_retry_notification();
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
            turn_runtime.try_recover_after_stream_error(
                &turn_context.session_telemetry,
                turn_context.model_info(),
            )
        },
    )
    .await
}

/// Handles a retryable model-stream error and returns `Ok(())` when the caller should retry.
///
/// `try_backend_recovery` performs a backend-private recovery action after the normal retry budget
/// is exhausted. A successful recovery may return a backend-specific warning to emit, keeping the
/// concrete recovery mechanism out of the generic retry policy.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_retryable_model_stream_error<F>(
    retry_state: &mut ModelStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    sess: &Session,
    turn_context: &TurnContext,
    request: ModelStreamRequest,
    allow_unbounded_connection_retry: bool,
    suppress_first_retry_notification: bool,
    mut try_backend_recovery: F,
) -> Result<(), CodexErr>
where
    F: FnMut() -> Option<String>,
{
    if allow_unbounded_connection_retry
        && turn_context
            .config
            .features
            .enabled(Feature::UnboundedConnectionRetries)
        && matches!(request, ModelStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ConnectionFailed(_))
        && !turn_context.session_source.is_internal()
    {
        let retry_delay = retry_state.connection_retry_delay;
        warn!(
            turn_id = %turn_context.sub_id,
            error = %err,
            ?retry_delay,
            "stream connection failed; waiting to retry"
        );
        sess.notify_stream_error(turn_context, "Reconnecting... waiting for network", err)
            .await;
        retry_state.connection_retries = retry_state.connection_retries.saturating_add(1);
        record_model_retry(retry_state.connection_retries, retry_delay, request);
        tokio::time::sleep(retry_delay).await;
        retry_state.connection_retry_delay = retry_delay
            .saturating_mul(2)
            .min(MAX_CONNECTION_RETRY_DELAY);
        return Ok(());
    }

    if retry_state.retries >= max_retries
        && let Some(recovery_warning) = try_backend_recovery()
    {
        sess.send_event(
            turn_context,
            EventMsg::Warning(WarningEvent {
                message: format!("{recovery_warning} {err:#}"),
            }),
        )
        .await;
        retry_state.retries = 0;
        return Ok(());
    }

    if retry_state.retries < max_retries {
        retry_state.retries += 1;
        let retry_count = retry_state.retries;
        let delay = err.retry_delay().unwrap_or_else(|| backoff(retry_count));
        log_retry(request, turn_context, &err, retry_count, max_retries, delay);

        let report_error =
            retry_count > 1 || cfg!(debug_assertions) || !suppress_first_retry_notification;
        if report_error {
            sess.notify_stream_error(
                turn_context,
                format!("Reconnecting... {retry_count}/{max_retries}"),
                err,
            )
            .await;
        }
        record_model_retry(retry_count, delay, request);
        tokio::time::sleep(delay).await;
        return Ok(());
    }

    Err(err)
}

fn record_model_retry(attempt: u64, delay: Duration, request: ModelStreamRequest) {
    let operation = match request {
        ModelStreamRequest::Sampling => "sampling",
        ModelStreamRequest::RemoteCompactionV2 => "remote_compaction_v2",
    };

    tracing::event!(
        target: "codex_otel.trace_safe",
        tracing::Level::TRACE,
        event.name = "codex.retry",
        retry.attempt = attempt,
        retry.delay_ms = delay.as_millis() as u64,
        retry.layer = "stream",
        retry.operation = operation,
    );
}

pub(crate) fn log_retry(
    request: ModelStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ModelStreamRequest::Sampling => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                sampling_error = %err,
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ModelStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
