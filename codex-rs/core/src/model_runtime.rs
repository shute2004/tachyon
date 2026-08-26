//! Transitional model-runtime boundary used while extracting Tachyon from Codex.
//!
//! This module deliberately preserves the existing Codex request/response surface for the first
//! extraction step. Types such as [`Prompt`], [`CodexResponsesMetadata`], and [`ResponseStream`]
//! are migration-only here; they are not the canonical Tachyon model IR.
//!
//! The important boundary introduced by this module is lifetime and ownership:
//!
//! - [`ModelRuntime`] is session-scoped.
//! - [`ModelTurnRuntime`] is a fresh handle for each harness turn.
//! - Fresh turn-affinity state stays private to [`ModelTurnRuntime`].
//! - A turn runtime may temporarily hold opaque backend state checked out from [`ModelRuntime`]
//!   when that state is reusable across turns.
//!
//! The current implementation is a thin adapter over `ModelClient` / `ModelClientSession`. It is
//! intentionally behavior-preserving so the mature Codex transport, retry, prewarm, continuation,
//! and routing behavior can be decomposed behind this seam later instead of being rewritten now.

use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::client_common::ResponseStream;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::Result;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::InferenceTraceContext;

/// Session-scoped model execution runtime.
///
/// This is not durable conversation state. Harness history remains owned by the Tachyon/Codex
/// session above this boundary.
#[derive(Debug, Clone)]
pub struct ModelRuntime {
    client: ModelClient,
}

impl ModelRuntime {
    /// Wraps the current Codex model client without changing its behavior.
    ///
    /// This constructor is part of the extraction bridge, not a commitment that providers will be
    /// represented by `ModelClient` in the standalone Tachyon architecture.
    pub fn from_codex_client(client: ModelClient) -> Self {
        Self { client }
    }

    /// Creates a fresh turn-scoped runtime handle.
    ///
    /// The handle gets fresh provider-private turn-affinity state, while the wrapped Codex
    /// implementation may also check out opaque reusable backend state from the session runtime.
    /// Reusable state is returned by `ModelClientSession::drop`; it must not be modeled as generic
    /// Tachyon fields merely because it is temporarily owned by the turn handle.
    pub fn begin_turn(&self) -> ModelTurnRuntime {
        ModelTurnRuntime::from_codex_session(self.client.new_session())
    }
}

/// Opaque model execution handle scoped to one harness turn.
///
/// The adapter intentionally does not expose `x-codex-turn-state`, `previous_response_id`, the
/// Responses WebSocket object, or incremental request bookkeeping. `x-codex-turn-state` is fresh
/// per turn; other opaque backend state may be reusable across turns even when it is temporarily
/// held by this handle. Those lifetime details remain private to the current OpenAI/Codex adapter.
pub struct ModelTurnRuntime {
    session: ModelClientSession,
}

impl ModelTurnRuntime {
    fn from_codex_session(session: ModelClientSession) -> Self {
        Self { session }
    }

    /// Streams one model request using the existing Codex implementation.
    ///
    /// The parameter types remain Responses-shaped during this first migration step. They are
    /// explicitly transitional and will be replaced by provider-neutral Tachyon request/event IR
    /// in a later extraction phase.
    #[allow(clippy::too_many_arguments)]
    pub async fn stream(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        self.session
            .stream(
                prompt,
                model_info,
                session_telemetry,
                effort,
                summary,
                service_tier,
                responses_metadata,
                inference_trace,
            )
            .await
    }

    /// Prepares this turn runtime using the existing Codex preparation implementation.
    ///
    /// Runtime preparation is the generic optional capability. The current OpenAI/Codex adapter
    /// realizes it with a Responses WebSocket `generate=false` warmup, but that mechanism is not
    /// part of the Tachyon contract and another backend may prepare differently or do nothing.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
    ) -> Result<()> {
        self.session
            .prewarm_websocket(
                prompt,
                model_info,
                session_telemetry,
                effort,
                summary,
                service_tier,
                responses_metadata,
            )
            .await
    }
}
