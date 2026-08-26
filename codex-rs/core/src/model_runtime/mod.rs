//! Tachyon model-runtime boundary.
//!
//! The public types in this module describe harness-level model execution lifetimes. The current
//! implementation is still backed by Codex/OpenAI behavior, which is isolated in
//! `codex_adapter` while the extraction is in progress.
//!
//! Request/response types in the method signatures are still migration-only Codex/Responses
//! shapes. They are not the canonical Tachyon model IR.

mod codex_adapter;
pub(crate) mod retry;

use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClient;
use crate::client_common::Prompt;
use crate::client_common::ResponseStream;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_adapter::CodexModelRuntimeAdapter;
use codex_adapter::CodexModelTurnRuntimeAdapter;
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
/// Durable harness conversation history remains owned above this boundary. The runtime may retain
/// opaque backend resources and recovery state that are reusable across turns.
#[derive(Debug, Clone)]
pub struct ModelRuntime {
    adapter: CodexModelRuntimeAdapter,
}

impl ModelRuntime {
    /// Wraps the transitional Codex model backend without treating `ModelClient` as Tachyon's
    /// canonical model abstraction.
    pub fn from_codex_client(client: ModelClient) -> Self {
        Self {
            adapter: CodexModelRuntimeAdapter::new(client),
        }
    }

    /// Creates a fresh execution handle for one harness turn.
    pub fn begin_turn(&self) -> ModelTurnRuntime {
        ModelTurnRuntime {
            adapter: self.adapter.begin_turn(),
        }
    }

    /// Returns whether startup preparation should produce a turn runtime that is transferred into
    /// the first harness turn. The concrete reason remains adapter-private.
    pub(crate) fn startup_preparation_uses_turn_runtime(&self) -> bool {
        self.adapter.startup_preparation_uses_turn_runtime()
    }

    /// Performs session-scoped startup preparation when no prepared turn runtime is produced.
    pub(crate) async fn prepare_session(&self) -> Result<()> {
        self.adapter.prepare_session().await
    }
}

/// Opaque model execution handle scoped to one harness turn.
///
/// Fresh turn-affinity state remains private to the backend. Reusable backend state may be checked
/// out by this handle and returned to the session-scoped runtime when the handle is dropped.
pub struct ModelTurnRuntime {
    adapter: CodexModelTurnRuntimeAdapter,
}

impl ModelTurnRuntime {
    /// Streams one model request through the current transitional backend.
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
        self.adapter
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

    /// Optionally prepares backend resources or opaque execution state before regular inference.
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
        self.adapter
            .prepare(
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

    /// Runs migration-stage remote compaction without exposing provider-private turn-affinity
    /// state to harness call sites.
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
        self.adapter
            .compact_conversation_history(
                prompt,
                model_info,
                settings,
                session_telemetry,
                compaction_trace,
                responses_metadata,
            )
            .await
    }

    /// Returns a backend-provided retry UX hint without tying it to runtime preparation semantics.
    pub(crate) fn suppress_first_retry_notification(&self) -> bool {
        self.adapter.suppress_first_retry_notification()
    }

    /// Lets the current backend attempt request-path recovery while retry policy remains above the
    /// runtime boundary. A successful recovery may return a backend-specific warning message.
    pub(crate) fn try_recover_after_stream_error(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> Option<String> {
        self.adapter
            .try_recover_after_stream_error(session_telemetry, model_info)
    }
}
