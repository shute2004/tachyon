use std::borrow::Cow;

use crate::RolloutItem;
use crate::protocol::EventMsg;
use codex_extension_items::ExtensionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SubAgentActivityKind;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_utils_string::truncate_middle_with_marker;

const PERSISTED_COMMAND_OUTPUT_MAX_BYTES: usize = 64 * 1024;
const COMMAND_OUTPUT_TRUNCATION_MARKER: &str =
    "\n... command output truncated for persistence ...\n";
const PERSISTED_MCP_TOOL_RESULT_MAX_BYTES: usize = 64 * 1024;
const PERSISTED_MCP_TOOL_RESULT_PREVIEW_MAX_BYTES: usize =
    (PERSISTED_MCP_TOOL_RESULT_MAX_BYTES - 128) / 2;
const MCP_TOOL_RESULT_TRUNCATION_MARKER: &str =
    "\n... MCP tool result truncated for persistence ...\n";

/// Whether a rollout `item` should be persisted in rollout files.
pub fn is_persisted_rollout_item(item: &RolloutItem, history_mode: ThreadHistoryMode) -> bool {
    match item {
        RolloutItem::ResponseItem(item) => should_persist_response_item(&item.item),
        RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. } => true,
        RolloutItem::EventMsg(ev) => should_persist_event_msg(ev, history_mode),
        RolloutItem::RealtimeItem(_) => matches!(history_mode, ThreadHistoryMode::Paginated),
        // Persist Codex executive markers so we can analyze flows (e.g., compaction, API turns).
        RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::SecurityRiskScore(_)
        | RolloutItem::SessionMeta(_) => true,
    }
}

/// Return the persisted projection of a rollout item without changing the live item.
///
/// Paginated command output fields and serialized MCP tool results are capped independently at
/// 64 KiB. MCP results use a smaller text-preview budget so JSON escaping cannot push the
/// projected result back over its serialized cap. All other retained items are borrowed unchanged.
pub fn persisted_rollout_item(
    item: &RolloutItem,
    history_mode: ThreadHistoryMode,
) -> Option<Cow<'_, RolloutItem>> {
    if !is_persisted_rollout_item(item, history_mode) {
        return None;
    }

    let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = item else {
        return Some(Cow::Borrowed(item));
    };
    if !matches!(history_mode, ThreadHistoryMode::Paginated) {
        return Some(Cow::Borrowed(item));
    }
    match &event.item {
        TurnItem::CommandExecution(command) => {
            let output_is_oversized = [
                command.stdout.as_deref(),
                command.stderr.as_deref(),
                command.aggregated_output.as_deref(),
                command.formatted_output.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|output| output.len() > PERSISTED_COMMAND_OUTPUT_MAX_BYTES);
            if !output_is_oversized {
                return Some(Cow::Borrowed(item));
            }

            let mut projected = item.clone();
            if let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = &mut projected
                && let TurnItem::CommandExecution(command) = &mut event.item
            {
                truncate_command_output(&mut command.stdout);
                truncate_command_output(&mut command.stderr);
                truncate_command_output(&mut command.aggregated_output);
                truncate_command_output(&mut command.formatted_output);
            }
            Some(Cow::Owned(projected))
        }
        TurnItem::McpToolCall(tool_call) => {
            let Some(result) = &tool_call.result else {
                return Some(Cow::Borrowed(item));
            };
            let Ok(serialized_result) = serde_json::to_string(result) else {
                return Some(Cow::Borrowed(item));
            };
            if serialized_result.len() <= PERSISTED_MCP_TOOL_RESULT_MAX_BYTES {
                return Some(Cow::Borrowed(item));
            }

            // The preview contains serialized JSON plus the marker's newlines, so embedding it
            // as a JSON string can expand it by at most 2x (quotes, backslashes, and newlines).
            // Its conservative half-cap plus less than 128 bytes of fixed CallToolResult framing
            // keeps the projected result within 64 KiB and makes another projection idempotent.
            let preview = truncate_middle_with_marker(
                &serialized_result,
                PERSISTED_MCP_TOOL_RESULT_PREVIEW_MAX_BYTES,
                MCP_TOOL_RESULT_TRUNCATION_MARKER,
            );
            let projected_result = CallToolResult {
                content: vec![serde_json::json!({
                    "type": "text",
                    "text": preview,
                })],
                structured_content: None,
                is_error: result.is_error,
                meta: None,
            };

            let mut projected = item.clone();
            if let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = &mut projected
                && let TurnItem::McpToolCall(tool_call) = &mut event.item
            {
                tool_call.result = Some(projected_result);
            }
            Some(Cow::Owned(projected))
        }
        _ => Some(Cow::Borrowed(item)),
    }
}

fn truncate_command_output(output: &mut Option<String>) {
    if let Some(output) = output
        && output.len() > PERSISTED_COMMAND_OUTPUT_MAX_BYTES
    {
        *output = truncate_middle_with_marker(
            output,
            PERSISTED_COMMAND_OUTPUT_MAX_BYTES,
            COMMAND_OUTPUT_TRUNCATION_MARKER,
        );
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
pub(crate) mod tests;

/// Return the rollout items that should be persisted for a live append.
pub fn persisted_rollout_items(
    items: &[RolloutItem],
    history_mode: ThreadHistoryMode,
) -> Vec<RolloutItem> {
    items
        .iter()
        .filter_map(|item| persisted_rollout_item(item, history_mode).map(Cow::into_owned))
        .collect()
}

/// Return persisted rollout items, moving unchanged values and owning only projected items.
pub fn into_persisted_rollout_items(
    items: Vec<RolloutItem>,
    history_mode: ThreadHistoryMode,
) -> Vec<RolloutItem> {
    items
        .into_iter()
        .filter_map(|item| {
            let projected = match persisted_rollout_item(&item, history_mode)? {
                Cow::Borrowed(_) => None,
                Cow::Owned(projected) => Some(projected),
            };
            Some(projected.unwrap_or(item))
        })
        .collect()
}

/// Whether a `ResponseItem` should be persisted in rollout files.
#[inline]
pub fn should_persist_response_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::AdditionalTools { .. } => true,
        ResponseItem::CompactionTrigger { .. } | ResponseItem::Other => false,
    }
}

/// Whether a `ResponseItem` should be persisted for the memories.
#[inline]
pub fn should_persist_response_item_for_memories(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { role, .. } => role != "developer",
        ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether an `EventMsg` should be persisted in rollout files.
#[inline]
pub fn should_persist_event_msg(ev: &EventMsg, history_mode: ThreadHistoryMode) -> bool {
    match ev {
        EventMsg::ItemCompleted(event) => {
            // Paginated rollouts store TurnItems.
            // Legacy rollouts keep only items with no lossless raw ResponseItem or legacy
            // equivalent.
            matches!(history_mode, ThreadHistoryMode::Paginated)
                || matches!(
                    event.item,
                    TurnItem::Plan(_) | TurnItem::Extension(ExtensionItem::Sleep(_))
                )
                || matches!(
                    &event.item,
                    TurnItem::SubAgentActivity(item)
                        if item.kind == SubAgentActivityKind::Completed
                )
        }
        EventMsg::TokenCount(_)
        | EventMsg::ThreadGoalUpdated(_)
        | EventMsg::ThreadRolledBack(_)
        | EventMsg::TurnAborted(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::ThreadSettingsApplied(_) => true,

        // Only persist these legacy events when the thread's history mode is Legacy.
        // New, paginated rollouts persist ItemCompleted events with TurnItems.
        EventMsg::UserMessage(_)
        | EventMsg::AgentMessage(_)
        | EventMsg::AgentReasoning(_)
        | EventMsg::AgentReasoningRawContent(_)
        | EventMsg::EnteredReviewMode(_)
        | EventMsg::ExitedReviewMode(_)
        | EventMsg::PatchApplyEnd(_)
        | EventMsg::ContextCompacted(_)
        | EventMsg::McpToolCallEnd(_)
        | EventMsg::WebSearchEnd(_)
        | EventMsg::ImageGenerationEnd(_) => {
            matches!(history_mode, ThreadHistoryMode::Legacy)
        }
        EventMsg::SubAgentActivity(event) => {
            matches!(history_mode, ThreadHistoryMode::Legacy)
                && event.kind != SubAgentActivityKind::Completed
        }

        // Transient, non-durable events.
        EventMsg::Error(_)
        | EventMsg::ThreadQueueChanged(_)
        | EventMsg::GuardianAssessment(_)
        | EventMsg::ExecCommandEnd(_)
        | EventMsg::ViewImageToolCall(_)
        | EventMsg::CollabAgentSpawnEnd(_)
        | EventMsg::CollabAgentInteractionEnd(_)
        | EventMsg::CollabWaitingEnd(_)
        | EventMsg::CollabCloseEnd(_)
        | EventMsg::CollabResumeEnd(_)
        | EventMsg::DynamicToolCallRequest(_)
        | EventMsg::DynamicToolCallResponse(_)
        | EventMsg::Warning(_)
        | EventMsg::AuthRecoveryStarted(_)
        | EventMsg::AuthRecoveryCompleted(_)
        | EventMsg::GuardianWarning(_)
        | EventMsg::RealtimeConversationStarted(_)
        | EventMsg::RealtimeConversationSdp(_)
        | EventMsg::RealtimeConversationRealtime(_)
        | EventMsg::RealtimeConversationClosed(_)
        | EventMsg::SafetyBuffering(_)
        | EventMsg::ModelReroute(_)
        | EventMsg::ModelVerification(_)
        | EventMsg::TurnModerationMetadata(_)
        | EventMsg::AgentReasoningSectionBreak(_)
        | EventMsg::ReasoningRawContentSectionStarted(_)
        | EventMsg::RawResponseItem(_)
        | EventMsg::RawResponseCompleted(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::EnvironmentConnected(_)
        | EventMsg::EnvironmentDisconnected(_)
        | EventMsg::McpToolCallBegin(_)
        | EventMsg::ExecCommandBegin(_)
        | EventMsg::TerminalInteraction(_)
        | EventMsg::ExecCommandOutputDelta(_)
        | EventMsg::ExecApprovalRequest(_)
        | EventMsg::RequestPermissions(_)
        | EventMsg::RequestUserInput(_)
        | EventMsg::ElicitationRequest(_)
        | EventMsg::ApplyPatchApprovalRequest(_)
        | EventMsg::StreamError(_)
        | EventMsg::PatchApplyBegin(_)
        | EventMsg::PatchApplyUpdated(_)
        | EventMsg::TurnDiff(_)
        | EventMsg::RealtimeConversationListVoicesResponse(_)
        | EventMsg::McpStartupUpdate(_)
        | EventMsg::McpStartupComplete(_)
        | EventMsg::WebSearchBegin(_)
        | EventMsg::PlanUpdate(_)
        | EventMsg::ShutdownComplete
        | EventMsg::DeprecationNotice(_)
        | EventMsg::ItemStarted(_)
        | EventMsg::HookStarted(_)
        | EventMsg::HookCompleted(_)
        | EventMsg::AgentMessageContentDelta(_)
        | EventMsg::PlanDelta(_)
        | EventMsg::ReasoningContentDelta(_)
        | EventMsg::ReasoningRawContentDelta(_)
        | EventMsg::ImageGenerationBegin(_)
        | EventMsg::CollabAgentSpawnBegin(_)
        | EventMsg::CollabAgentInteractionBegin(_)
        | EventMsg::CollabWaitingBegin(_)
        | EventMsg::CollabCloseBegin(_)
        | EventMsg::CollabResumeBegin(_) => false,
    }
}
