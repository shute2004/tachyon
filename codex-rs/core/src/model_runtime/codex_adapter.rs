use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client::WebsocketStreamOutcome;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::model_runtime::codex_event::CodexEventMapper;
use crate::model_runtime::codex_event::ModelRuntimeEvent;
use crate::model_runtime::codex_request::prompt_from_model_request;
use crate::model_runtime::ir::ModelRequest;
use crate::model_runtime::route::ModelProtocol;
use crate::model_runtime::route::ModelProviderId;
use crate::model_runtime::route::ModelRoute;
use crate::model_runtime::route::ModelTransport;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_otel::SessionTelemetry;
use codex_otel::current_span_w3c_trace_context;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::CompactionTraceContext;
use codex_rollout_trace::InferenceTraceContext;

const FALLBACK_TO_HTTP_WARNING: &str = "Falling back from WebSockets to HTTPS transport.";
const OPENAI_RESPONSES_PROTOCOL_ID: &str = "openai.responses";

fn codex_transport(websocket_enabled: bool) -> ModelTransport {
    if websocket_enabled {
        ModelTransport::WebSocket
    } else {
        ModelTransport::Http
    }
}

fn codex_route(provider_id: ModelProviderId, websocket_enabled: bool) -> ModelRoute {
    let transport = if websocket_enabled {
        ModelTransport::WebSocket
    } else {
        ModelTransport::Http
    };
    ModelRoute::new(
        provider_id,
        ModelProtocol::new(OPENAI_RESPONSES_PROTOCOL_ID),
        transport,
    )
}

/// Transitional adapter over the current Codex session-scoped model client.
#[derive(Debug, Clone)]
pub(super) struct CodexModelRuntimeAdapter {
    client: ModelClient,
}

impl CodexModelRuntimeAdapter {
    pub(super) fn new(client: ModelClient) -> Self {
        Self { client }
    }

    pub(super) fn begin_turn_for_provider(
        &self,
        provider_id: ModelProviderId,
    ) -> CodexModelTurnRuntimeAdapter {
        CodexModelTurnRuntimeAdapter::new(self.client.clone(), provider_id)
    }

    fn current_transport(&self) -> ModelTransport {
        codex_transport(self.client.responses_websocket_enabled())
    }

    pub(super) fn startup_preparation_uses_turn_runtime(&self) -> bool {
        matches!(self.current_transport(), ModelTransport::WebSocket)
    }

    pub(super) async fn prepare_session(&self) -> Result<()> {
        self.client.prewarm_auth().await
    }
}

/// Transitional adapter over the current Codex turn-scoped model execution state.
pub(super) struct CodexModelTurnRuntimeAdapter {
    client: ModelClient,
    provider_id: ModelProviderId,
    session: ModelClientSession,
    event_mapper: CodexEventMapper,
}

impl CodexModelTurnRuntimeAdapter {
    fn new(client: ModelClient, provider_id: ModelProviderId) -> Self {
        let session = client.new_session();
        Self {
            client,
            provider_id,
            session,
            event_mapper: CodexEventMapper::default(),
        }
    }

    fn current_route(&self) -> ModelRoute {
        codex_route(
            self.provider_id.clone(),
            self.client.responses_websocket_enabled(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn stream(
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
        let route = self.current_route();

        match (route.protocol().id(), route.transport()) {
            (OPENAI_RESPONSES_PROTOCOL_ID, ModelTransport::Http) => {
                self.session
                    .stream_responses_api(
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
            (OPENAI_RESPONSES_PROTOCOL_ID, ModelTransport::WebSocket) => {
                let request_trace = current_span_w3c_trace_context();
                match self
                    .session
                    .stream_responses_websocket(
                        prompt,
                        model_info,
                        session_telemetry,
                        effort.clone(),
                        summary,
                        service_tier.clone(),
                        responses_metadata,
                        /*warmup*/ false,
                        request_trace,
                        inference_trace,
                    )
                    .await?
                {
                    WebsocketStreamOutcome::Stream(stream) => Ok(stream),
                    WebsocketStreamOutcome::FallbackToHttp => {
                        self.session
                            .try_switch_fallback_transport(session_telemetry, model_info);
                        self.session
                            .stream_responses_api(
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
                }
            }
            _ => unreachable!("Codex adapter resolved an unsupported model protocol"),
        }
    }

    /// Converts Tachyon's canonical request semantics back into the current Codex request shape at
    /// the adapter boundary. `legacy_prompt` is migration-only preservation state for provider-
    /// private item decorations that intentionally do not belong in `ModelRequest`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn stream_model_request(
        &mut self,
        request: &ModelRequest,
        legacy_prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let prompt = prompt_from_model_request(request, legacy_prompt)?;
        self.stream(
            &prompt,
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

    /// Maps one Codex/OpenAI stream event into canonical model semantics or the explicit
    /// compatibility side channel. Mapping state is turn-scoped and resets on each Created event.
    pub(super) fn map_stream_event(&mut self, event: ResponseEvent) -> ModelRuntimeEvent {
        self.event_mapper.map(event)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn prepare(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
    ) -> Result<()> {
        if !matches!(self.current_route().transport(), ModelTransport::WebSocket) {
            return Ok(());
        }

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

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn compact_conversation_history(
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

    pub(super) fn suppress_first_retry_notification(&self) -> bool {
        matches!(self.current_route().transport(), ModelTransport::WebSocket)
    }

    pub(super) fn try_recover_after_stream_error(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> Option<String> {
        if !matches!(self.current_route().transport(), ModelTransport::WebSocket) {
            return None;
        }

        self.session
            .try_switch_fallback_transport(session_telemetry, model_info)
            .then(|| FALLBACK_TO_HTTP_WARNING.to_string())
    }
}

#[cfg(test)]
#[path = "codex_route_tests.rs"]
mod tests;
