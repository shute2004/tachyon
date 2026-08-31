//! Lossless compatibility projection from Responses history items into the kernel vocabulary.
//!
//! The projection is deliberately conservative. A `HistoryItem` is produced only when the
//! corresponding Responses item has a direct provider-neutral meaning. The original envelope is
//! retained in either result so callers can continue to use provider IDs, metadata, and wire-only
//! fields while the history migration is in progress.

use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use serde_json::Value;

use crate::HistoryItem;
use crate::HistoryMediaSource;
use crate::HistoryMessage;
use crate::HistoryMessageContent;
use crate::HistoryMessagePhase;
use crate::HistoryMessageRole;
use crate::HistoryReasoning;
use crate::HistoryToolCall;
use crate::HistoryToolCallId;
use crate::HistoryToolInput;
use crate::HistoryToolResult;
use crate::HistoryToolResultContent;
use crate::ResponseItemEnvelope;

const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_CLIENT_EXECUTION: &str = "client";

/// Result of projecting one Responses item into the provider-neutral history vocabulary.
///
/// Both variants retain the exact owned source envelope. This is intentional: the canonical item
/// is useful to generic history code, while the compatibility envelope remains authoritative for
/// provider IDs, sidecar metadata, wire distinctions, and provider-private bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryItemProjection {
    Canonical {
        item: HistoryItem,
        compatibility: ResponseItemEnvelope,
    },
    Fallback {
        compatibility: ResponseItemEnvelope,
        reason: HistoryProjectionFallback,
    },
}

/// Why an item stayed on the Responses compatibility path.
///
/// The compatibility envelope contains the detailed source value, so these reasons intentionally
/// remain stable, semantic categories rather than duplicating provider-specific payload fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryProjectionFallback {
    UnknownMessageRole,
    EncryptedReasoning,
    MissingFunctionCallId,
    InvalidFunctionArguments,
    EncryptedFunctionArguments,
    MissingCustomToolCallId,
    MissingClientToolSearchCallId,
    ProviderToolSearchCall,
    MissingFunctionCallOutputCallId,
    MissingCustomToolCallOutputCallId,
    EncryptedToolResultContent,
    UnsupportedLocalShell,
    UnsupportedAgentMessage,
    UnsupportedAdditionalTools,
    UnsupportedCompaction,
    UnsupportedImageGeneration,
    UnsupportedWebSearch,
    UnsupportedToolSearchOutput,
    UnsupportedOther,
}

/// Project one Responses-shaped history envelope into a canonical item when its meaning is
/// representable without losing semantics.
pub fn project_response_item(source: ResponseItemEnvelope) -> HistoryItemProjection {
    let projection = project_item(&source.item);
    match projection {
        Ok(item) => HistoryItemProjection::Canonical {
            item,
            compatibility: source,
        },
        Err(reason) => HistoryItemProjection::Fallback {
            compatibility: source,
            reason,
        },
    }
}

fn project_item(item: &ResponseItem) -> Result<HistoryItem, HistoryProjectionFallback> {
    match item {
        ResponseItem::Message {
            role,
            content,
            phase,
            ..
        } => Ok(HistoryItem::Message(HistoryMessage {
            role: project_message_role(role)?,
            phase: phase.as_ref().map(project_message_phase),
            content: content
                .iter()
                .map(project_message_content)
                .collect::<Vec<_>>(),
        })),
        ResponseItem::Reasoning {
            summary,
            content,
            encrypted_content,
            ..
        } => {
            if encrypted_content.is_some() {
                return Err(HistoryProjectionFallback::EncryptedReasoning);
            }

            Ok(HistoryItem::Reasoning(HistoryReasoning {
                summary: summary.iter().map(project_reasoning_summary).collect(),
                content: content
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(project_reasoning_content)
                    .collect(),
            }))
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            encrypted_function_args,
            call_id,
            ..
        } => {
            if call_id.is_empty() {
                return Err(HistoryProjectionFallback::MissingFunctionCallId);
            }
            if encrypted_function_args
                .as_ref()
                .is_some_and(|parts| parts.iter().any(|part| !part.is_empty()))
            {
                return Err(HistoryProjectionFallback::EncryptedFunctionArguments);
            }
            let input = serde_json::from_str::<Value>(arguments)
                .map_err(|_| HistoryProjectionFallback::InvalidFunctionArguments)?;

            Ok(HistoryItem::ToolCall(HistoryToolCall {
                call_id: HistoryToolCallId(call_id.clone()),
                namespace: namespace.clone(),
                name: name.clone(),
                input: HistoryToolInput::Json(input),
            }))
        }
        ResponseItem::CustomToolCall {
            call_id,
            name,
            namespace,
            input,
            ..
        } => {
            if call_id.is_empty() {
                return Err(HistoryProjectionFallback::MissingCustomToolCallId);
            }

            Ok(HistoryItem::ToolCall(HistoryToolCall {
                call_id: HistoryToolCallId(call_id.clone()),
                namespace: namespace.clone(),
                name: name.clone(),
                input: HistoryToolInput::Text(input.clone()),
            }))
        }
        ResponseItem::ToolSearchCall {
            call_id,
            execution,
            arguments,
            ..
        } => {
            if execution != TOOL_SEARCH_CLIENT_EXECUTION {
                return Err(HistoryProjectionFallback::ProviderToolSearchCall);
            }
            let Some(call_id) = call_id.as_ref().filter(|call_id| !call_id.is_empty()) else {
                return Err(HistoryProjectionFallback::MissingClientToolSearchCallId);
            };

            Ok(HistoryItem::ToolCall(HistoryToolCall {
                call_id: HistoryToolCallId(call_id.clone()),
                namespace: None,
                name: TOOL_SEARCH_NAME.to_string(),
                input: HistoryToolInput::Json(arguments.clone()),
            }))
        }
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => {
            let Some(call_id) = call_id.as_ref().filter(|call_id| !call_id.is_empty()) else {
                return Err(HistoryProjectionFallback::MissingFunctionCallOutputCallId);
            };

            Ok(HistoryItem::ToolResult(HistoryToolResult {
                call_id: HistoryToolCallId(call_id.clone()),
                content: project_tool_result_content(output)?,
                is_error: output.success.map(|success| !success),
            }))
        }
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            if call_id.is_empty() {
                return Err(HistoryProjectionFallback::MissingCustomToolCallOutputCallId);
            }

            Ok(HistoryItem::ToolResult(HistoryToolResult {
                call_id: HistoryToolCallId(call_id.clone()),
                content: project_tool_result_content(output)?,
                is_error: output.success.map(|success| !success),
            }))
        }
        ResponseItem::LocalShellCall { .. } => {
            Err(HistoryProjectionFallback::UnsupportedLocalShell)
        }
        ResponseItem::AgentMessage { .. } => {
            Err(HistoryProjectionFallback::UnsupportedAgentMessage)
        }
        ResponseItem::AdditionalTools { .. } => {
            Err(HistoryProjectionFallback::UnsupportedAdditionalTools)
        }
        ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. } => {
            Err(HistoryProjectionFallback::UnsupportedCompaction)
        }
        ResponseItem::ImageGenerationCall { .. } => {
            Err(HistoryProjectionFallback::UnsupportedImageGeneration)
        }
        ResponseItem::WebSearchCall { .. } => Err(HistoryProjectionFallback::UnsupportedWebSearch),
        ResponseItem::ToolSearchOutput { .. } => {
            Err(HistoryProjectionFallback::UnsupportedToolSearchOutput)
        }
        ResponseItem::Other => Err(HistoryProjectionFallback::UnsupportedOther),
    }
}

fn project_message_role(role: &str) -> Result<HistoryMessageRole, HistoryProjectionFallback> {
    match role {
        "system" => Ok(HistoryMessageRole::System),
        "developer" => Ok(HistoryMessageRole::Developer),
        "user" => Ok(HistoryMessageRole::User),
        "assistant" => Ok(HistoryMessageRole::Assistant),
        _ => Err(HistoryProjectionFallback::UnknownMessageRole),
    }
}

fn project_message_phase(phase: &MessagePhase) -> HistoryMessagePhase {
    match phase {
        MessagePhase::Commentary => HistoryMessagePhase::Commentary,
        MessagePhase::FinalAnswer => HistoryMessagePhase::Final,
    }
}

fn project_message_content(content: &ContentItem) -> HistoryMessageContent {
    match content {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            HistoryMessageContent::Text(text.clone())
        }
        ContentItem::InputImage { image_url, detail } => HistoryMessageContent::Image {
            source: HistoryMediaSource::Uri(image_url.clone()),
            detail: detail.as_ref().map(project_image_detail),
        },
        ContentItem::InputAudio { audio_url } => HistoryMessageContent::Audio {
            source: HistoryMediaSource::Uri(audio_url.clone()),
        },
    }
}

fn project_image_detail(detail: &ImageDetail) -> crate::HistoryImageDetail {
    match detail {
        ImageDetail::Auto => crate::HistoryImageDetail::Auto,
        ImageDetail::Low => crate::HistoryImageDetail::Low,
        ImageDetail::High => crate::HistoryImageDetail::High,
        ImageDetail::Original => crate::HistoryImageDetail::Original,
    }
}

fn project_reasoning_summary(summary: &ReasoningItemReasoningSummary) -> String {
    match summary {
        ReasoningItemReasoningSummary::SummaryText { text } => text.clone(),
    }
}

fn project_reasoning_content(content: &ReasoningItemContent) -> String {
    match content {
        ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
            text.clone()
        }
    }
}

fn project_tool_result_content(
    output: &codex_protocol::models::FunctionCallOutputPayload,
) -> Result<Vec<HistoryToolResultContent>, HistoryProjectionFallback> {
    match &output.body {
        FunctionCallOutputBody::Text(text) => {
            Ok(vec![HistoryToolResultContent::Text(text.clone())])
        }
        FunctionCallOutputBody::ContentItems(items) => items
            .iter()
            .map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => {
                    Ok(HistoryToolResultContent::Text(text.clone()))
                }
                FunctionCallOutputContentItem::InputImage { image_url, detail } => {
                    Ok(HistoryToolResultContent::Image {
                        source: HistoryMediaSource::Uri(image_url.clone()),
                        detail: detail.as_ref().map(project_image_detail),
                    })
                }
                FunctionCallOutputContentItem::InputAudio { audio_url } => {
                    Ok(HistoryToolResultContent::Audio {
                        source: HistoryMediaSource::Uri(audio_url.clone()),
                    })
                }
                FunctionCallOutputContentItem::EncryptedContent { .. } => {
                    Err(HistoryProjectionFallback::EncryptedToolResultContent)
                }
            })
            .collect(),
    }
}
