//! Tachyon model-runtime boundary.
//!
//! The public types in this module describe harness-level model execution lifetimes. The current
//! implementation is still backed by Codex/OpenAI behavior, which is isolated in
//! `codex_adapter` while the extraction is in progress.
//!
//! Canonical provider-neutral request/event vocabulary lives in `ir`. C2 routes representable
//! regular sampling requests through `ModelRequest`; C3 maps representable stream events into
//! `ModelEvent` while retaining an explicit compatibility side channel for Codex/Responses-only
//! event semantics and product/backend notifications. D1 moved protocol/transport selection into
//! the model-runtime adapter. D2 introduced provider identity as an independent route dimension.
//! D3 binds configured provider identity for every turn-scoped runtime. Session startup capability
//! checks remain adapter-private and do not construct a model route before provider binding.

mod codex_adapter;
mod codex_event;
mod codex_request;
pub mod ir;
pub(crate) mod retry;
pub mod route;
mod tool_result;

use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClient;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_adapter::CodexModelRuntimeAdapter;
use codex_adapter::CodexModelTurnRuntimeAdapter;
pub(crate) use codex_event::CodexModelEventContext;
pub(crate) use codex_event::CodexModelRuntimeSideEvent;
pub(crate) use codex_event::ModelRuntimeEvent;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::CompactionTraceContext;
use codex_rollout_trace::InferenceTraceContext;
use ir::ModelRequest;
use route::ModelProviderId;
pub(crate) use tool_result::to_response_item as tool_result_to_response_item;

/// Transitional C2 bridge: project the current Codex prompt into canonical request semantics when
/// doing so is lossless. Unsupported provider-specific history/state stays on the legacy path.
pub(crate) fn try_model_request_from_prompt(prompt: &Prompt) -> Option<ModelRequest> {
    codex_request::try_model_request_from_prompt(prompt)
}

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

    /// Creates a fresh execution handle for one harness turn bound to the selected provider.
    ///
    /// The provider identity is opaque to the runtime and remains independent from protocol,
    /// endpoint, authentication, and transport selection.
    pub fn begin_turn_for_provider(&self, provider_id: impl Into<String>) -> ModelTurnRuntime {
        ModelTurnRuntime {
            adapter: self
                .adapter
                .begin_turn_for_provider(ModelProviderId::new(provider_id.into())),
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

    /// Transitional C2 entry point for regular sampling.
    ///
    /// Representable requests are sent through the canonical `ModelRequest` conversion boundary.
    /// Requests that still contain unsupported Codex/Responses-only semantics use the legacy
    /// `Prompt` path unchanged. The legacy template is also used below the boundary to restore
    /// provider-private item decorations that deliberately do not belong in the canonical IR.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn stream_migrating_request(
        &mut self,
        request: Option<&ModelRequest>,
        legacy_prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        match request {
            Some(request) => {
                self.adapter
                    .stream_model_request(
                        request,
                        legacy_prompt,
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
            None => {
                self.adapter
                    .stream(
                        legacy_prompt,
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
        }
    }

    /// Converts one current Codex/OpenAI stream event into the C3 runtime event boundary.
    ///
    /// Generic model semantics become `ModelEvent`; provider/product data and unsupported model
    /// shapes remain on the explicit compatibility side channel until their ownership is resolved.
    pub(crate) fn map_stream_event(&mut self, event: ResponseEvent) -> ModelRuntimeEvent {
        self.adapter.map_stream_event(event)
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
    /// state to harness call sites. Representable model-visible request semantics cross the
    /// canonical `ModelRequest` boundary; unsupported Codex/Responses-only shapes retain the
    /// existing legacy path unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn compact_conversation_history_migrating_request(
        &self,
        request: Option<&ModelRequest>,
        legacy_prompt: &Prompt,
        model_info: &ModelInfo,
        settings: CompactConversationRequestSettings,
        session_telemetry: &SessionTelemetry,
        compaction_trace: &CompactionTraceContext,
        responses_metadata: &CodexResponsesMetadata,
    ) -> Result<Vec<ResponseItem>> {
        match request {
            Some(request) => {
                self.adapter
                    .compact_model_request(
                        request,
                        legacy_prompt,
                        model_info,
                        settings,
                        session_telemetry,
                        compaction_trace,
                        responses_metadata,
                    )
                    .await
            }
            None => {
                self.adapter
                    .compact_conversation_history(
                        legacy_prompt,
                        model_info,
                        settings,
                        session_telemetry,
                        compaction_trace,
                        responses_metadata,
                    )
                    .await
            }
        }
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
