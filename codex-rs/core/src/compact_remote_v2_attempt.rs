use std::sync::Arc;

use super::MAX_REMOTE_COMPACTION_V2_STREAM_RETRIES;
use super::RemoteCompactionV2Output;
use super::collect_compaction_output;
use super::run_remote_compaction_request_v2;
use crate::Prompt;
use crate::client::ModelClientSession;
use crate::compact::CompactionAnalyticsDetails;
use crate::compact_remote::trim_function_call_history_to_fit_context_window;
use crate::model_runtime::ModelTurnRuntime;
use crate::model_runtime::retry::ModelStreamRequest;
use crate::model_runtime::retry::ModelStreamRetryState;
use crate::model_runtime::retry::handle_retryable_turn_runtime_error;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CompactionTurnMetadata;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use codex_history::CodexHarnessMetadata;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RawResponseCompletedEvent;
use codex_protocol::protocol::TokenUsage;
use codex_rollout_trace::CompactionTraceContext;
use codex_rollout_trace::InferenceTraceContext;
use tracing::info;

pub(super) struct RemoteCompactV2Attempt {
    pub(super) trace_input_history: Option<Vec<ResponseItem>>,
    pub(super) prompt_input: Vec<ResponseItem>,
    pub(super) prompt_input_metadata: Vec<Option<CodexHarnessMetadata>>,
    pub(super) compaction_output: ResponseItem,
    pub(super) token_usage: Option<TokenUsage>,
    /// Keeps a runtime created for standalone compaction alive through lifecycle completion.
    pub(super) owned_turn_runtime: Option<ModelTurnRuntime>,
}

pub(super) async fn run_remote_compact_v2_attempt(
    sess: &Arc<Session>,
    step_context: &Arc<StepContext>,
    client_session: Option<&mut ModelClientSession>,
    compaction_trace: &CompactionTraceContext,
    compaction_metadata: CompactionTurnMetadata,
    analytics_details: &mut CompactionAnalyticsDetails,
) -> CodexResult<RemoteCompactV2Attempt> {
    let turn_context = &step_context.turn;
    let mut history = sess.clone_history().await;
    let base_instructions = sess.get_base_instructions().await;
    let (rewritten_outputs, estimated_deleted_tokens) =
        trim_function_call_history_to_fit_context_window(
            &mut history,
            turn_context.as_ref(),
            &base_instructions,
        );
    if rewritten_outputs > 0 {
        info!(
            turn_id = %turn_context.sub_id,
            rewritten_outputs,
            "rewrote history outputs before remote compaction v2"
        );
    }
    if estimated_deleted_tokens > 0 {
        let max_local_deleted_tokens = sess
            .estimated_tokens_after_last_model_generated_item()
            .await;
        analytics_details.active_context_tokens_before = analytics_details
            .active_context_tokens_before
            .map(|active_context_tokens_before| {
                active_context_tokens_before
                    .saturating_sub(estimated_deleted_tokens.min(max_local_deleted_tokens))
            });
    }

    let trace_input_history = compaction_trace
        .is_enabled()
        .then(|| history.raw_items().cloned().collect());
    let (mut input, prompt_input_metadata): (Vec<_>, Vec<_>) = history
        .for_prompt_annotated(&turn_context.model_info().input_modalities)
        .into_iter()
        .map(|envelope| (envelope.item, envelope.metadata))
        .unzip();
    let tool_router = &step_context.tool_router;
    input.push(ResponseItem::CompactionTrigger {});
    let prompt = Prompt {
        input,
        tools: tool_router.model_visible_specs(),
        parallel_tool_calls: true,
        base_instructions,
        output_schema: None,
        output_schema_strict: true,
        cyber_access_program: turn_context.cyber_access_program,
    };

    let responses_metadata = sess
        .responses_metadata(
            turn_context.as_ref(),
            CodexResponsesRequestKind::Compaction(compaction_metadata),
        )
        .await;
    let trace_attempt = compaction_trace.start_attempt(&serde_json::json!({
        "model": turn_context.model_info().slug.as_str(),
        "instructions": prompt.base_instructions.text.as_str(),
        "input": &prompt.input,
        "parallel_tool_calls": prompt.parallel_tool_calls,
    }));
    let mut owned_turn_runtime = None;
    let compaction_output_result = match client_session {
        Some(client_session) => {
            run_remote_compaction_request_v2(
                sess,
                turn_context.as_ref(),
                client_session,
                &prompt,
                &responses_metadata,
            )
            .await
        }
        None => {
            let turn_runtime =
                owned_turn_runtime.insert(sess.services.model_runtime().begin_turn());
            run_remote_compaction_request_v2_with_turn_runtime(
                sess,
                turn_context.as_ref(),
                turn_runtime,
                &prompt,
                &responses_metadata,
            )
            .await
        }
    };
    trace_attempt.record_result(
        compaction_output_result
            .as_ref()
            .map(|output| std::slice::from_ref(&output.compaction_output)),
    );
    let RemoteCompactionV2Output {
        compaction_output,
        response_id,
        token_usage,
    } = compaction_output_result?;
    // TODO: Emit this before compaction output validation so malformed completed
    // responses still surface their raw upstream usage.
    sess.send_event(
        turn_context,
        EventMsg::RawResponseCompleted(RawResponseCompletedEvent {
            response_id,
            token_usage: token_usage.clone(),
        }),
    )
    .await;
    let mut prompt_input = prompt.input;
    prompt_input.pop();
    Ok(RemoteCompactV2Attempt {
        trace_input_history,
        prompt_input,
        prompt_input_metadata,
        compaction_output,
        token_usage,
        owned_turn_runtime,
    })
}

async fn run_remote_compaction_request_v2_with_turn_runtime(
    sess: &Session,
    turn_context: &crate::session::turn_context::TurnContext,
    turn_runtime: &mut ModelTurnRuntime,
    prompt: &Prompt,
    responses_metadata: &CodexResponsesMetadata,
) -> CodexResult<RemoteCompactionV2Output> {
    let max_retries = turn_context
        .provider
        .info()
        .stream_max_retries()
        .min(MAX_REMOTE_COMPACTION_V2_STREAM_RETRIES);
    let mut retry_state = ModelStreamRetryState::default();
    loop {
        let result = match turn_runtime
            .stream(
                prompt,
                turn_context.model_info(),
                &turn_context.session_telemetry,
                turn_context.reasoning_effort().cloned(),
                turn_context.reasoning_summary(),
                turn_context.config.service_tier.clone(),
                responses_metadata,
                &InferenceTraceContext::disabled(),
            )
            .await
        {
            Ok(stream) => collect_compaction_output(stream).await,
            Err(err) => Err(err),
        };

        match result {
            Ok(compaction_output) => return Ok(compaction_output),
            Err(err) if !err.is_retryable() => return Err(err),
            Err(err) => {
                handle_retryable_turn_runtime_error(
                    &mut retry_state,
                    max_retries,
                    err,
                    turn_runtime,
                    sess,
                    turn_context,
                    ModelStreamRequest::RemoteCompactionV2,
                )
                .await?;
            }
        }
    }
}
