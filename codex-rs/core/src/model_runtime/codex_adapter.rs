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

const FALLBACK_TO_HTTP_WARNING: &str = "Falling back from WebSockets to HTTPS transport.";

/// Transitional adapter over the current Codex session-scoped model client.
#[derive(Debug, Clone)]
pub(super) struct CodexModelRuntimeAdapter {
    client: ModelClient,
}

impl CodexModelRuntimeAdapter {
    pub(super) fn new(client: ModelClient) -> Self {
        Self { client }
    }

    pub(super) fn begin_turn(&self) -> CodexModelTurnRuntimeAdapter {
        CodexModelTurnRuntimeAdapter::new(self.client.clone())
    }
}

/// Transitional adapter over the current Codex turn-scoped model execution state.
pub(super) struct CodexModelTurnRuntimeAdapter {
    client: ModelClient,
    session: ModelClientSession,
}

impl CodexModelTurnRuntimeAdapter {
    fn new(client: ModelClient) -> Self {
        let session = client.new_session();
        Self { client, session }
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
        self.client.responses_websocket_enabled()
    }

    pub(super) fn try_recover_after_stream_error(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> Option<String> {
        self.session
            .try_switch_fallback_transport(session_telemetry, model_info)
            .then(|| FALLBACK_TO_HTTP_WARNING.to_string())
    }
}
