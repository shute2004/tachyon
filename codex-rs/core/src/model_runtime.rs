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

use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::client_common::ResponseStream;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::CompactionTraceContext;
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
        ModelTurnRuntime::from_codex_client(self.client.clone())
    }

    /// Returns whether the current transitional adapter has a turn-runtime preparation path.
    ///
    /// The present Codex implementation maps this to Responses WebSocket preparation. The
    /// capability name intentionally describes the generic lifecycle rather than that transport.
    pub(crate) fn has_turn_preparation(&self) -> bool {
        self.client.responses_websocket_enabled()
    }

    /// Performs session-level preparation when no prepared turn runtime will be produced.
    ///
    /// Today this resolves Codex/OpenAI authentication so Agent Identity bootstrap behavior stays
    /// unchanged. It remains crate-private while the provider-neutral preparation contract is
    /// still being extracted.
    pub(crate) async fn prepare_session(&self) -> Result<()> {
        self.client.prewarm_auth().await
    }
}

/// Opaque model execution handle scoped to one harness turn.
///
/// The adapter intentionally does not expose `x-codex-turn-state`, `previous_response_id`, the
/// Responses WebSocket object, or incremental request bookkeeping. `x-codex-turn-state` is fresh
/// per turn; other opaque backend state may be reusable across turns even when it is temporarily
/// held by this handle. Those lifetime details remain private to the current OpenAI/Codex adapter.
pub struct ModelTurnRuntime {
    /// Session-scoped adapter handle used by capabilities that are implemented outside the
    /// streaming `ModelClientSession` API during migration. This clone shares the same underlying
    /// `ModelClientState` as `session`.
    client: ModelClient,
    session: ModelClientSession,
}

impl ModelTurnRuntime {
    fn from_codex_client(client: ModelClient) -> Self {
        let session = client.new_session();
        Self { client, session }
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

    /// Runs the existing remote-compaction request while keeping provider-private turn affinity
    /// state inside the runtime boundary.
    ///
    /// This is a migration-only capability surface: request/response types are still Codex and
    /// Responses shaped, but callers no longer need to extract `x-codex-turn-state` themselves.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn compact_conversation_history(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        settings: CompactConversationRequestSettings,
        session_telemetry: &SessionTelemetry,
        compaction_trace: &CompactionTraceContext,
        responses_metadata: &CodexResponsesMetadata,
    ) -> Result<Vec<ResponseItem>> {
        self.client
            .compact_conversation_history(
                prompt,
                model_info,
                Some(self.session.turn_state()),
                settings,
                session_telemetry,
                compaction_trace,
                responses_metadata,
            )
            .await
    }

    /// Lets the current adapter attempt request-path recovery without exposing its concrete
    /// WebSocket-to-HTTP mechanism as public Tachyon API.
    ///
    /// Retry budgets and scheduling remain harness concerns; this crate-private bridge only
    /// delegates the backend-specific recovery action during the migration.
    pub(crate) fn try_recover_after_stream_error(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> bool {
        self.session
            .try_switch_fallback_transport(session_telemetry, model_info)
    }
}
