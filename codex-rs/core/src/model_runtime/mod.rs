//! Tachyon model-runtime boundary.
//!
//! The public types in this module describe harness-level model execution lifetimes. The current
//! default implementation preserves Codex/OpenAI behavior behind `codex_adapter`. A session may
//! select a canonical `ModelBackend` for regular sampling while the extraction is in progress.
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
mod harness_event;
mod historical_selection;
pub mod ir;
pub(crate) mod retry;
pub mod route;
mod tool_result;

use std::sync::Arc;

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
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::CompactionTraceContext;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use futures::future::poll_fn;
pub(crate) use harness_event::HarnessSamplingEvent;
pub(crate) use harness_event::normalize_sampling_event;
pub(crate) use historical_selection::HistoricalModelSelection;
use ir::ModelRequest;
use route::ModelProviderId;
use tachyon_model::ModelEvent;
use tachyon_model::backend::ModelBackend;
use tachyon_model::backend::ModelBackendError;
use tachyon_model::backend::ModelEventStream;
use tachyon_model::backend::ModelTurnBackend;
pub(crate) use tool_result::to_response_item as tool_result_to_response_item;

/// Migration-only bridge from the current Codex turn context into prior-turn metadata.
pub(crate) fn historical_model_selection_from_codex_turn_context(
    turn_context: &crate::session::turn_context::TurnContext,
) -> HistoricalModelSelection {
    codex_adapter::historical_model_selection_from_codex_turn_context(turn_context)
}

/// Migration-only bridge from the existing serialized Codex turn context item into prior-turn
/// metadata. The model and provider-private program are taken atomically from that item.
pub(crate) fn historical_model_selection_from_codex_turn_context_item(
    turn_context: &codex_protocol::protocol::TurnContextItem,
) -> HistoricalModelSelection {
    codex_adapter::historical_model_selection_from_codex_turn_context_item(turn_context)
}

/// Migration-only bridge restoring a prior model selection onto a cloned Codex turn context.
pub(crate) async fn codex_turn_context_for_historical_selection(
    current: &crate::session::turn_context::TurnContext,
    selection: &HistoricalModelSelection,
    models_manager: &codex_models_manager::manager::SharedModelsManager,
) -> crate::session::turn_context::TurnContext {
    codex_adapter::turn_context_for_historical_selection(current, selection, models_manager).await
}

/// Migration-only bridge constructing a local compaction prompt from its selected turn context.
pub(crate) fn codex_local_compaction_prompt(
    input: Vec<ResponseItem>,
    base_instructions: codex_protocol::models::BaseInstructions,
    turn_context: &crate::session::turn_context::TurnContext,
) -> Prompt {
    codex_adapter::local_compaction_prompt(input, base_instructions, turn_context)
}

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
    backend: ModelRuntimeBackend,
}

#[derive(Debug, Clone)]
enum ModelRuntimeBackend {
    Codex(CodexModelRuntimeAdapter),
    Canonical(Arc<dyn ModelBackend>),
}

impl ModelRuntime {
    /// Wraps the transitional Codex model backend without treating `ModelClient` as Tachyon's
    /// canonical model abstraction.
    pub fn from_codex_client(client: ModelClient) -> Self {
        Self {
            backend: ModelRuntimeBackend::Codex(CodexModelRuntimeAdapter::new(client)),
        }
    }

    /// Creates a session-scoped runtime backed by a provider-neutral model backend.
    pub fn from_backend(backend: Arc<dyn ModelBackend>) -> Self {
        Self {
            backend: ModelRuntimeBackend::Canonical(backend),
        }
    }

    /// Reports whether this runtime can execute the existing Codex remote-compaction path.
    pub(crate) fn supports_codex_remote_compaction(&self) -> bool {
        matches!(&self.backend, ModelRuntimeBackend::Codex(_))
    }

    /// Creates a fresh execution handle for one harness turn bound to the selected provider.
    ///
    /// The provider identity is opaque to the runtime and remains independent from protocol,
    /// endpoint, authentication, and transport selection.
    pub fn begin_turn_for_provider(&self, provider_id: impl Into<String>) -> ModelTurnRuntime {
        let provider_id = ModelProviderId::new(provider_id.into());
        let backend = match &self.backend {
            ModelRuntimeBackend::Codex(adapter) => {
                ModelTurnRuntimeBackend::Codex(adapter.begin_turn_for_provider(provider_id))
            }
            ModelRuntimeBackend::Canonical(backend) => ModelTurnRuntimeBackend::Canonical {
                backend: backend.begin_turn(provider_id.clone()),
                requested_provider_id: provider_id,
            },
        };
        ModelTurnRuntime { backend }
    }

    /// Returns whether startup preparation should produce a turn runtime that is transferred into
    /// the first harness turn. The concrete reason remains adapter-private.
    pub(crate) fn startup_preparation_uses_turn_runtime(&self) -> bool {
        match &self.backend {
            ModelRuntimeBackend::Codex(adapter) => adapter.startup_preparation_uses_turn_runtime(),
            ModelRuntimeBackend::Canonical(_) => false,
        }
    }

    /// Performs session-scoped startup preparation when no prepared turn runtime is produced.
    pub(crate) async fn prepare_session(&self) -> Result<()> {
        match &self.backend {
            ModelRuntimeBackend::Codex(adapter) => adapter.prepare_session().await,
            ModelRuntimeBackend::Canonical(backend) => backend
                .prepare_session()
                .await
                .map_err(model_backend_error_to_codex),
        }
    }

    /// Creates an independent runtime for a delegated child session when the backend supports it.
    ///
    /// The Codex adapter keeps its existing child-session behavior, which constructs a fresh
    /// legacy client in the child session. Canonical backends must return a separate session
    /// factory rather than sharing this runtime's backend instance as the child's affinity owner.
    pub(crate) async fn new_child_session(&self) -> Result<Option<Self>> {
        match &self.backend {
            ModelRuntimeBackend::Codex(_) => Ok(None),
            ModelRuntimeBackend::Canonical(backend) => backend
                .new_child_session()
                .await
                .map(Self::from_backend)
                .map(Some)
                .map_err(model_backend_error_to_codex),
        }
    }
}

/// Opaque model execution handle scoped to one harness turn.
///
/// Fresh turn-affinity state remains private to the backend. Reusable backend state may be checked
/// out by this handle and returned to the session-scoped runtime when the handle is dropped.
pub struct ModelTurnRuntime {
    backend: ModelTurnRuntimeBackend,
}

enum ModelTurnRuntimeBackend {
    Codex(CodexModelTurnRuntimeAdapter),
    Canonical {
        backend: Box<dyn ModelTurnBackend>,
        requested_provider_id: ModelProviderId,
    },
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
        match &mut self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => {
                adapter
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
            ModelTurnRuntimeBackend::Canonical { .. } => Err(CodexErr::InvalidRequest(
                "canonical model backends require a provider-neutral ModelRequest".to_string(),
            )),
        }
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
    ) -> Result<SamplingModelStream> {
        match &mut self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => match request {
                Some(request) => adapter
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
                    .map(SamplingModelStream::Codex),
                None => adapter
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
                    .map(SamplingModelStream::Codex),
            },
            ModelTurnRuntimeBackend::Canonical {
                backend,
                requested_provider_id,
            } => {
                let actual_provider_id = backend.route().provider_id();
                if actual_provider_id != requested_provider_id {
                    return Err(CodexErr::InvalidRequest(format!(
                        "configured model provider `{}` does not match model backend route provider `{}`",
                        requested_provider_id.id(),
                        actual_provider_id.id(),
                    )));
                }
                let request = request.ok_or_else(|| {
                    CodexErr::InvalidRequest(
                        "canonical model backend cannot execute this turn because its prompt contains Codex-only or otherwise unsupported request data".to_string(),
                    )
                })?;
                backend
                    .stream(&model_info.slug, request)
                    .await
                    .map(SamplingModelStream::Canonical)
                    .map_err(model_backend_error_to_codex)
            }
        }
    }

    /// Converts one current Codex/OpenAI stream event into the C3 runtime event boundary.
    ///
    /// Generic model semantics become `ModelEvent`; provider/product data and unsupported model
    /// shapes remain on the explicit compatibility side channel until their ownership is resolved.
    pub(crate) fn map_stream_event(&mut self, event: ResponseEvent) -> ModelRuntimeEvent {
        let ModelTurnRuntimeBackend::Codex(adapter) = &mut self.backend else {
            unreachable!("canonical model streams do not contain Codex ResponseEvents")
        };
        adapter.map_stream_event(event)
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
        match &mut self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => {
                adapter
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
            ModelTurnRuntimeBackend::Canonical { .. } => Err(CodexErr::InvalidRequest(
                "canonical model backends do not accept Codex prompt preparation".to_string(),
            )),
        }
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
        match &self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => match request {
                Some(request) => {
                    adapter
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
                    adapter
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
            },
            ModelTurnRuntimeBackend::Canonical { .. } => Err(CodexErr::InvalidRequest(
                "canonical model backend does not support Codex remote compaction".to_string(),
            )),
        }
    }

    /// Returns a backend-provided retry UX hint without tying it to runtime preparation semantics.
    pub(crate) fn suppress_first_retry_notification(&self) -> bool {
        match &self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => adapter.suppress_first_retry_notification(),
            ModelTurnRuntimeBackend::Canonical { .. } => false,
        }
    }

    /// Lets the current backend attempt request-path recovery while retry policy remains above the
    /// runtime boundary. A successful recovery may return a backend-specific warning message.
    pub(crate) fn try_recover_after_stream_error(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> Option<String> {
        match &mut self.backend {
            ModelTurnRuntimeBackend::Codex(adapter) => {
                adapter.try_recover_after_stream_error(session_telemetry, model_info)
            }
            ModelTurnRuntimeBackend::Canonical { .. } => None,
        }
    }
}

pub(crate) enum SamplingModelStream {
    Codex(ResponseStream),
    Canonical(ModelEventStream),
}

pub(crate) enum SamplingModelStreamEvent {
    Codex(ResponseEvent),
    Canonical(ModelEvent),
}

impl SamplingModelStream {
    pub(crate) async fn next_event(&mut self) -> Option<Result<SamplingModelStreamEvent>> {
        match self {
            Self::Codex(stream) => stream
                .next()
                .await
                .map(|event| event.map(SamplingModelStreamEvent::Codex)),
            Self::Canonical(stream) => {
                poll_fn(|cx| stream.as_mut().poll_next(cx))
                    .await
                    .map(|event| {
                        event
                            .map_err(model_backend_error_to_codex)
                            .map(SamplingModelStreamEvent::Canonical)
                    })
            }
        }
    }

    pub(crate) fn closed_stream_error(&self) -> CodexErr {
        match self {
            Self::Codex(_) => CodexErr::Stream("stream closed before response.completed".into()),
            Self::Canonical(_) => CodexErr::InvalidRequest(
                "canonical model event stream closed before Completed".into(),
            ),
        }
    }
}

pub(crate) fn model_backend_error_to_codex(error: ModelBackendError) -> CodexErr {
    match error {
        ModelBackendError::Failed {
            message,
            retryable: true,
        } => CodexErr::Stream(message),
        ModelBackendError::Failed {
            message,
            retryable: false,
        } => CodexErr::InvalidRequest(message),
        ModelBackendError::UnsupportedRequest(message) => {
            CodexErr::InvalidRequest(format!("unsupported canonical model request: {message}"))
        }
        ModelBackendError::Cancelled => CodexErr::TurnAborted,
    }
}
