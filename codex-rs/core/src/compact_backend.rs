use std::borrow::Borrow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::Prompt;
use crate::compact::CompactedHistoryMetadata;
use crate::compact::CompactionAnalyticsAttempt;
use crate::compact::CompactionAnalyticsDetails;
use crate::compact::InitialContextInjection;
use crate::compact::build_compaction_initial_context;
use crate::compact::compaction_status_from_result;
use crate::context_manager::HistoryToolSide;
use crate::context_manager::estimate_item_token_count;
use crate::context_manager::tool_correlation;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::model_runtime::ModelTurnRuntime;
use crate::model_runtime::response_item_from_new_model_input;
use crate::model_runtime::response_item_from_new_model_tool_result;
use crate::model_runtime::token_usage_from_model;
use crate::model_runtime::try_model_request_from_prompt;
use crate::responses_metadata::CompactionTurnMetadata;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionStatus;
use codex_analytics::CompactionTrigger;
use codex_history::ResponseItemEnvelope;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnStartedEvent;
use tachyon_model::ModelInputItem;
use tachyon_model::ModelRequest;
use tachyon_model::backend::ModelCompactionItem;
use tachyon_model::backend::ModelCompactionResult;
use tokio_util::sync::CancellationToken;

pub(crate) async fn run_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> CodexResult<()> {
    let step_context = sess
        .capture_step_context(Arc::clone(&turn_context), &CancellationToken::new())
        .await?;
    let start_event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        trace_id: turn_context.trace_id.clone(),
        started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.mode(),
    });
    sess.send_event(&turn_context, start_event).await;

    let mut turn_runtime = sess
        .services
        .model_runtime()
        .begin_turn_for_provider(turn_context.config.model_provider_id.clone());
    run_compact_task_inner(
        &sess,
        &step_context,
        &mut turn_runtime,
        InitialContextInjection::DoNotInject,
        CompactionTurnMetadata::new(
            CompactionTrigger::Manual,
            CompactionReason::UserRequested,
            CompactionImplementation::Backend,
            CompactionPhase::StandaloneTurn,
        ),
    )
    .await
}

pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    step_context: Arc<StepContext>,
    turn_runtime: &mut ModelTurnRuntime,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    run_compact_task_inner(
        &sess,
        &step_context,
        turn_runtime,
        initial_context_injection,
        CompactionTurnMetadata::new(
            CompactionTrigger::Auto,
            reason,
            CompactionImplementation::Backend,
            phase,
        ),
    )
    .await
}

async fn run_compact_task_inner(
    sess: &Arc<Session>,
    step_context: &Arc<StepContext>,
    turn_runtime: &mut ModelTurnRuntime,
    initial_context_injection: InitialContextInjection,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<()> {
    let turn_context = &step_context.turn;
    let trigger = compaction_metadata.trigger();
    let reason = compaction_metadata.reason();
    let implementation = compaction_metadata.implementation();
    let phase = compaction_metadata.phase();
    let mut analytics_details = CompactionAnalyticsDetails {
        active_context_tokens_before: Some(sess.get_total_token_usage().await),
        ..Default::default()
    };
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        turn_context.as_ref(),
        trigger,
        reason,
        implementation,
        phase,
    )
    .await;
    match run_pre_compact_hooks(sess, turn_context, trigger).await {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped => {
            let error = CodexErr::TurnAborted;
            attempt
                .track(
                    sess.as_ref(),
                    CompactionStatus::Interrupted,
                    Some(&error),
                    analytics_details,
                )
                .await;
            return Err(error);
        }
    }

    let result = run_compact_task_inner_impl(
        sess,
        step_context,
        turn_runtime,
        initial_context_injection,
        &mut analytics_details,
    )
    .await;
    let status = compaction_status_from_result(&result);
    let codex_error = result.as_ref().err();
    if result.is_ok()
        && let PostCompactHookOutcome::Stopped =
            run_post_compact_hooks(sess, turn_context, trigger).await
    {
        attempt
            .track(sess.as_ref(), status, codex_error, analytics_details)
            .await;
        return Err(CodexErr::TurnAborted);
    }
    attempt
        .track(sess.as_ref(), status, codex_error, analytics_details)
        .await;
    match result {
        Ok(()) => Ok(()),
        Err(err) if matches!(err.details(), CodexErrorDetails::TurnAborted) => Err(err),
        Err(err) => {
            sess.track_turn_codex_error(turn_context, &err);
            let event = EventMsg::Error(
                err.to_error_event(Some("Error running backend compact task".to_string())),
            );
            sess.send_event(turn_context, event).await;
            Err(err)
        }
    }
}

async fn run_compact_task_inner_impl(
    sess: &Arc<Session>,
    step_context: &Arc<StepContext>,
    turn_runtime: &mut ModelTurnRuntime,
    initial_context_injection: InitialContextInjection,
    analytics_details: &mut CompactionAnalyticsDetails,
) -> CodexResult<()> {
    let turn_context = &step_context.turn;
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    sess.emit_turn_item_started(turn_context, &compaction_item)
        .await;

    let history = sess.clone_history().await;
    let snapshot = history.for_prompt_annotated(&turn_context.model_info().input_modalities);
    let prompt = Prompt {
        input: snapshot
            .iter()
            .map(|envelope| envelope.item.clone())
            .collect(),
        tools: step_context.tool_router.model_visible_specs(),
        parallel_tool_calls: true,
        base_instructions: sess.get_base_instructions().await,
        output_schema: None,
        output_schema_strict: true,
        cyber_access_program: turn_context.cyber_access_program,
    };
    let model_request = try_model_request_from_prompt(&prompt).ok_or_else(|| {
        CodexErr::InvalidRequest(
            "canonical model backend cannot compact this turn because its normalized history contains Codex-only or otherwise unsupported request data".to_string(),
        )
    })?;
    if model_request.input.len() != snapshot.len() {
        return Err(CodexErr::InvalidRequest(
            "canonical model compaction request does not preserve normalized history slot positions".to_string(),
        ));
    }

    let result = turn_runtime
        .compact_backend_request(&turn_context.model_info().slug, &model_request)
        .await?;
    let prepared =
        prepare_backend_compaction(&snapshot, &model_request, result, &turn_context.sub_id)?;
    if let Some(token_usage) = prepared.token_usage.as_ref() {
        analytics_details.active_context_tokens_before = Some(token_usage.input_tokens);
        analytics_details.compaction_summary_tokens = Some(token_usage.output_tokens);
        analytics_details.cached_input_tokens = Some(token_usage.cached_input_tokens);
        analytics_details.cache_write_input_tokens = Some(token_usage.cache_write_input_tokens);
        sess.update_token_usage_info(turn_context, Some(token_usage))
            .await?;
    }

    let (window_number, window_ids) = sess.advance_auto_compact_window().await;
    let (mut new_history, world_state_baseline) =
        build_compaction_initial_context(sess.as_ref(), &initial_context_injection).await;
    new_history.extend(prepared.history);
    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage { .. } => {
            Some(turn_context.to_turn_context_item())
        }
    };
    sess.replace_compacted_history(
        new_history,
        reference_context_item,
        world_state_baseline,
        CompactedHistoryMetadata {
            message: String::new(),
            window_number,
            window_ids,
        },
    )
    .await;
    sess.recompute_token_usage(turn_context).await;
    sess.emit_turn_item_completed(turn_context, compaction_item)
        .await;
    Ok(())
}

struct PreparedBackendCompaction {
    history: Vec<ResponseItemEnvelope>,
    token_usage: Option<TokenUsage>,
}

const MAX_NEW_BACKEND_COMPACTION_ITEM_TOKENS: i64 = 10_000;

/// Applies the existing coarse model-visible history estimate to fresh backend output.
/// This is a model-visible history heuristic, not an exact provider tokenizer.
fn new_backend_compaction_envelope(
    mut item: ResponseItem,
    turn_id: &str,
) -> CodexResult<ResponseItemEnvelope> {
    item.set_turn_id_if_missing(turn_id);
    let estimated_tokens = estimate_item_token_count(&item);
    if estimated_tokens > MAX_NEW_BACKEND_COMPACTION_ITEM_TOKENS {
        return Err(CodexErr::InvalidRequest(format!(
            "backend compaction item exceeds the {MAX_NEW_BACKEND_COMPACTION_ITEM_TOKENS}-token estimated item limit ({estimated_tokens} estimated tokens; estimate is approximate)"
        )));
    }
    Ok(ResponseItemEnvelope::new(item))
}

/// Temporarily projects backend compaction output into the current Codex history envelope.
/// Canonical items without a lossless Codex response-item representation are rejected.
fn prepare_backend_compaction(
    snapshot: &[ResponseItemEnvelope],
    request: &ModelRequest,
    result: ModelCompactionResult,
    turn_id: &str,
) -> CodexResult<PreparedBackendCompaction> {
    if request.input.len() != snapshot.len() {
        return Err(CodexErr::InvalidRequest(
            "canonical model compaction result cannot be mapped to normalized history slots"
                .to_string(),
        ));
    }
    result
        .validate_retained_inputs(snapshot.len())
        .map_err(crate::model_runtime::model_backend_error_to_codex)?;
    validate_retained_groups(snapshot, &result.history)?;
    let token_usage = result
        .usage
        .map(token_usage_from_model)
        .transpose()
        .map_err(|error| {
            CodexErr::InvalidRequest(format!(
                "backend compaction returned invalid token usage: {error}"
            ))
        })?;

    let group_indexes = history_group_indexes(snapshot);
    let mut projected_new_calls = HashMap::new();
    let mut projected_calls_by_id: HashMap<String, Vec<ResponseItem>> = HashMap::new();
    for (output_index, item) in result.history.iter().enumerate() {
        let projected_call = match item {
            ModelCompactionItem::RetainedInput { index } => {
                let (_, _, group_len) = group_indexes[*index];
                let envelope = &snapshot[*index];
                let reinjectable_context = group_len == 1
                    && envelope.metadata.as_ref().is_some_and(|metadata| {
                        metadata.reinjectable_context
                            && !metadata.client_authored
                            && metadata.input_association.is_none()
                    });
                if reinjectable_context {
                    None
                } else {
                    tool_correlation(&envelope.item)
                        .filter(|correlation| correlation.side == HistoryToolSide::Call)
                        .map(|_| envelope.item.clone())
                }
            }
            ModelCompactionItem::NewItem(ModelInputItem::ToolCall(call)) => {
                let item = ModelInputItem::ToolCall(call.clone());
                let response_item = response_item_from_new_model_input(&item, &request.tools)
                    .map_err(|error| {
                        CodexErr::InvalidRequest(format!(
                            "backend compaction returned a tool call that cannot be represented in Codex history: {error}"
                        ))
                    })?;
                projected_new_calls.insert(output_index, response_item.clone());
                Some(response_item)
            }
            ModelCompactionItem::NewItem(_) => None,
        };
        if let Some(call) = projected_call
            && let Some(correlation) = tool_correlation(&call)
            && correlation.side == HistoryToolSide::Call
        {
            projected_calls_by_id
                .entry(correlation.call_id.to_string())
                .or_default()
                .push(call);
        }
    }

    let mut history = Vec::with_capacity(result.history.len());
    for (output_index, item) in result.history.into_iter().enumerate() {
        match item {
            ModelCompactionItem::RetainedInput { index } => {
                let (_, _, group_len) = group_indexes[index];
                let envelope = &snapshot[index];
                let reinjectable_context = group_len == 1
                    && envelope.metadata.as_ref().is_some_and(|metadata| {
                        metadata.reinjectable_context
                            && !metadata.client_authored
                            && metadata.input_association.is_none()
                    });
                if !reinjectable_context {
                    history.push(envelope.clone());
                }
            }
            ModelCompactionItem::NewItem(ModelInputItem::ToolCall(_)) => {
                let item = projected_new_calls.remove(&output_index).ok_or_else(|| {
                    CodexErr::InvalidRequest(
                        "backend compaction tool call lost its projected history item".to_string(),
                    )
                })?;
                history.push(new_backend_compaction_envelope(item, turn_id)?);
            }
            ModelCompactionItem::NewItem(ModelInputItem::ToolResult(result)) => {
                let calls = projected_calls_by_id
                    .get(&result.call_id.0)
                    .ok_or_else(|| {
                        CodexErr::InvalidRequest(format!(
                            "backend compaction returned tool result `{}` without a projected call",
                            result.call_id.0
                        ))
                    })?;
                let [call] = calls.as_slice() else {
                    return Err(CodexErr::InvalidRequest(format!(
                        "backend compaction returned tool result `{}` with ambiguous projected calls",
                        result.call_id.0
                    )));
                };
                let item = response_item_from_new_model_tool_result(&result, call).map_err(
                    |error| {
                        CodexErr::InvalidRequest(format!(
                            "backend compaction returned a tool result that cannot be represented in Codex history: {error}"
                        ))
                    },
                )?;
                history.push(new_backend_compaction_envelope(item, turn_id)?);
            }
            ModelCompactionItem::NewItem(item) => {
                let item = response_item_from_new_model_input(&item, &request.tools)
                    .map_err(|error| {
                        CodexErr::InvalidRequest(format!(
                            "backend compaction returned an item that cannot be represented in Codex history: {error}"
                        ))
                    })?;
                history.push(new_backend_compaction_envelope(item, turn_id)?);
            }
        }
    }
    validate_tool_correlations(&history)?;
    Ok(PreparedBackendCompaction {
        history,
        token_usage,
    })
}

fn validate_tool_correlations(history: &[ResponseItemEnvelope]) -> CodexResult<()> {
    let mut calls = HashSet::new();
    let mut outputs = HashSet::new();
    let mut required_outputs = HashSet::new();
    for envelope in history {
        let Some(correlation) = tool_correlation(&envelope.item) else {
            continue;
        };
        let key = (correlation.compatibility_pairing_class, correlation.call_id);
        match correlation.side {
            HistoryToolSide::Call => {
                calls.insert(key);
            }
            HistoryToolSide::Output => {
                outputs.insert(key);
                if correlation.local_counterpart_required {
                    required_outputs.insert(key);
                }
            }
        }
    }

    for (pairing_class, call_id) in &calls {
        if !outputs.contains(&(*pairing_class, *call_id)) {
            return Err(CodexErr::InvalidRequest(format!(
                "backend compaction returned tool call `{call_id}` without a matching output in pairing class {pairing_class:?}"
            )));
        }
    }
    for (pairing_class, call_id) in &required_outputs {
        if !calls.contains(&(*pairing_class, *call_id)) {
            return Err(CodexErr::InvalidRequest(format!(
                "backend compaction returned tool output `{call_id}` without a matching call in pairing class {pairing_class:?}"
            )));
        }
    }
    Ok(())
}

fn validate_retained_groups(
    snapshot: &[ResponseItemEnvelope],
    output: &[ModelCompactionItem],
) -> CodexResult<()> {
    let group_indexes = history_group_indexes(snapshot);
    let mut output_index = 0;
    while output_index < output.len() {
        match &output[output_index] {
            ModelCompactionItem::RetainedInput { index } => {
                let (_, group_position, group_len) = group_indexes[*index];
                if group_position != 0 {
                    return Err(CodexErr::InvalidRequest(format!(
                        "backend compaction retained attached history notice at input index {index} without its source"
                    )));
                }
                if group_len == 2 {
                    // history_item_groups attaches only the immediately following normalized
                    // slot, and the pair must remain adjacent and in source/notice order.
                    let expected_notice = *index + 1;
                    if !matches!(
                        output.get(output_index + 1),
                        Some(ModelCompactionItem::RetainedInput { index }) if *index == expected_notice
                    ) {
                        return Err(CodexErr::InvalidRequest(format!(
                            "backend compaction must retain source and attached notice together at input index {index}"
                        )));
                    }
                    output_index += 1;
                }
            }
            ModelCompactionItem::NewItem(_) => {}
        }
        output_index += 1;
    }
    Ok(())
}

fn history_group_indexes(snapshot: &[ResponseItemEnvelope]) -> Vec<(usize, usize, usize)> {
    let indexed = snapshot
        .iter()
        .enumerate()
        .map(|(index, envelope)| IndexedHistoryItem {
            index,
            item: &envelope.item,
        });
    let mut indexes = vec![(0, 0, 0); snapshot.len()];
    for (group_index, group) in
        crate::compact_remote_history::history_item_groups(indexed).enumerate()
    {
        let group_indexes = group
            .into_items()
            .map(|item| item.index)
            .collect::<Vec<_>>();
        for (position, index) in group_indexes.iter().copied().enumerate() {
            indexes[index] = (group_index, position, group_indexes.len());
        }
    }
    indexes
}

struct IndexedHistoryItem<'a> {
    index: usize,
    item: &'a ResponseItem,
}

impl Borrow<ResponseItem> for IndexedHistoryItem<'_> {
    fn borrow(&self) -> &ResponseItem {
        self.item
    }
}

#[cfg(test)]
#[path = "compact_backend_tests.rs"]
mod tests;
