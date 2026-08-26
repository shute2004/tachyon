//! Transitional model-runtime boundary used while extracting Tachyon from Codex.
//!
//! This module deliberately preserves the existing Codex request/response surface for the first
//! extraction step. Types such as [`Prompt`], [`CodexResponsesMetadata`], and [`ResponseStream`]
//! are migration-only here; they are not the canonical Tachyon model IR.
//!
//! The important boundary introduced by this module is lifetime and ownership:
//!
//! - [`ModelRuntime`] is session-scoped.
//! - [`ModelTurnRuntime`] is fresh for each harness turn.
//! - Provider-private turn state stays inside [`ModelTurnRuntime`].
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
    pub fn from_codex_client(client: ModelClient) -> Self {
        Self { client }
    }

    /// Creates a fresh turn-scoped runtime.
    ///
    /// A new value must be created for every harness turn. Reusing a turn runtime across turns
    /// would also reuse provider-private state such as the current OpenAI sticky-routing token.
    pub fn begin_turn(&self) -> ModelTurnRuntime {
        ModelTurnRuntime::from_codex_session(self.client.new_session())
    }

    /// Returns whether the current adapter can use its Responses WebSocket transport.
    ///
    /// This method exists only to preserve startup-prewarm behavior during the migration. The
    /// Responses-specific capability will move behind a provider-neutral preparation capability.
    pub fn responses_websocket_enabled(&self) -> bool {
        self.client.responses_websocket_enabled()
    }

    /// Performs the existing authentication prewarm path.
    pub async fn prewarm_auth(&self) -> Result<()> {
        self.client.prewarm_auth().await
    }
}

impl From<ModelClient> for ModelRuntime {
    fn from(client: ModelClient) -> Self {
        Self::from_codex_client(client)
    }
}

/// Opaque model execution state scoped to one harness turn.
///
/// The adapter intentionally does not expose `x-codex-turn-state`, `previous_response_id`, the
/// Responses WebSocket object, or incremental request bookkeeping. Those remain implementation
/// details of the current OpenAI/Codex adapter.
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

    /// Runs the existing Responses WebSocket prewarm for this turn runtime.
    ///
    /// The generic capability is runtime preparation; the concrete WebSocket operation remains an
    /// adapter detail and will be generalized after the seam is wired through the agent loop.
    #[allow(clippy::too_many_arguments)]
    pub async fn prewarm(
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

    /// Activates the current adapter's transport fallback policy.
    ///
    /// The generic retry policy remains above the model runtime; the concrete WebSocket-to-HTTP
    /// mechanism remains behind this boundary.
    pub fn try_switch_fallback_transport(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> bool {
        self.session
            .try_switch_fallback_transport(session_telemetry, model_info)
    }

    /// Temporary bridge for Codex subsystems that still take `ModelClientSession` directly.
    ///
    /// This is crate-private on purpose. It allows remote compaction and retry call sites to move
    /// incrementally without making provider-private state part of Tachyon's public runtime
    /// contract. New code outside the migration should not depend on it.
    pub(crate) fn legacy_session_mut(&mut self) -> &mut ModelClientSession {
        &mut self.session
    }
}