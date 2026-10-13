//! Compact primary-history storage with a lossless Responses compatibility boundary.
//!
//! Canonical records keep model semantics once and only the small wire decorations needed to
//! reconstruct a Responses item. Items whose semantics cannot be represented exactly remain on
//! the legacy path until a later compatibility migration handles them.

use std::borrow::Cow;

use codex_history::CodexHarnessMetadata;
use codex_history::HistoryItem;
use codex_history::HistoryMessageContent;
use codex_history::HistoryMessagePhase;
use codex_history::HistoryToolResultContent;
use codex_history::ResponseItemEnvelope;
use codex_history::project_response_item_semantics;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;

#[cfg(test)]
#[path = "history_record_tests.rs"]
mod tests;

/// One primary-history record. The Responses sidecar is deliberately kept outside the payload.
#[derive(Debug, Clone)]
pub(super) struct StoredHistoryRecord {
    payload: StoredPayload,
    metadata: Option<CodexHarnessMetadata>,
}

#[derive(Debug, Clone)]
enum StoredPayload {
    Canonical {
        item: HistoryItem,
        decoration: ResponsesDecoration,
    },
    Legacy(ResponseItem),
}

/// Responses-only structure and identity that is not represented in the shared semantic item.
/// This enum intentionally contains no message, reasoning, tool input, or output body text.
#[derive(Debug, Clone)]
enum ResponsesDecoration {
    Message {
        id: Option<ResponseItemId>,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
        text_kinds: Vec<TextContentKind>,
    },
    Reasoning {
        id: Option<ResponseItemId>,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
        content_kinds: Option<Vec<ReasoningContentKind>>,
    },
    FunctionCall {
        id: Option<ResponseItemId>,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
    },
    CustomToolCall {
        id: Option<ResponseItemId>,
        status: Option<String>,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
    },
    FunctionCallOutput {
        id: Option<ResponseItemId>,
        name: Option<String>,
        namespace: Option<String>,
        body_kind: ToolOutputBodyKind,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
    },
    CustomToolCallOutput {
        id: Option<ResponseItemId>,
        name: Option<String>,
        body_kind: ToolOutputBodyKind,
        passthrough: Option<InternalChatMessageMetadataPassthrough>,
    },
}

#[derive(Debug, Clone, Copy)]
enum TextContentKind {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy)]
enum ReasoningContentKind {
    ReasoningText,
    Text,
}

#[derive(Debug, Clone, Copy)]
enum ToolOutputBodyKind {
    Text,
    ContentItems,
}

impl StoredHistoryRecord {
    pub(super) fn from_response_envelope(envelope: ResponseItemEnvelope) -> Self {
        let payload = match canonical_payload(&envelope.item) {
            Some((item, decoration)) => StoredPayload::Canonical { item, decoration },
            None => StoredPayload::Legacy(envelope.item),
        };
        Self {
            payload,
            metadata: envelope.metadata,
        }
    }

    pub(super) fn response_item(&self) -> Cow<'_, ResponseItem> {
        match &self.payload {
            StoredPayload::Legacy(item) => Cow::Borrowed(item),
            StoredPayload::Canonical { item, decoration } => {
                Cow::Owned(reconstruct_response_item(item, decoration))
            }
        }
    }

    pub(super) fn into_response_envelope(self) -> ResponseItemEnvelope {
        let item = match self.payload {
            StoredPayload::Legacy(item) => item,
            StoredPayload::Canonical { item, decoration } => {
                reconstruct_response_item(&item, &decoration)
            }
        };
        ResponseItemEnvelope {
            item,
            metadata: self.metadata,
        }
    }

    pub(super) fn response_envelope(&self) -> ResponseItemEnvelope {
        ResponseItemEnvelope {
            item: self.response_item().into_owned(),
            metadata: self.metadata.clone(),
        }
    }
}

fn canonical_payload(item: &ResponseItem) -> Option<(HistoryItem, ResponsesDecoration)> {
    // Tool search is a host-specific compatibility operation even when a semantic projection is
    // possible. Encrypted arguments likewise remain byte-for-byte on the legacy path.
    if matches!(item, ResponseItem::ToolSearchCall { .. })
        || matches!(
            item,
            ResponseItem::FunctionCall {
                encrypted_function_args: Some(_),
                ..
            }
        )
    {
        return None;
    }

    if let ResponseItem::FunctionCall { arguments, .. } = item {
        let parsed = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
        if serde_json::to_string(&parsed).ok()?.as_bytes() != arguments.as_bytes() {
            return None;
        }
    }

    let semantic = project_response_item_semantics(item).ok()?;
    let decoration = match item {
        ResponseItem::Message {
            id,
            content,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::Message {
            id: id.clone(),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
            text_kinds: content
                .iter()
                .filter_map(|part| match part {
                    ContentItem::InputText { .. } => Some(TextContentKind::Input),
                    ContentItem::OutputText { .. } => Some(TextContentKind::Output),
                    ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => None,
                })
                .collect(),
        },
        ResponseItem::Reasoning {
            id,
            content,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::Reasoning {
            id: id.clone(),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
            content_kinds: content.as_ref().map(|parts| {
                parts
                    .iter()
                    .map(|part| match part {
                        ReasoningItemContent::ReasoningText { .. } => {
                            ReasoningContentKind::ReasoningText
                        }
                        ReasoningItemContent::Text { .. } => ReasoningContentKind::Text,
                    })
                    .collect()
            }),
        },
        ResponseItem::FunctionCall {
            id,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::FunctionCall {
            id: id.clone(),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
        },
        ResponseItem::CustomToolCall {
            id,
            status,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::CustomToolCall {
            id: id.clone(),
            status: status.clone(),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
        },
        ResponseItem::FunctionCallOutput {
            id,
            name,
            namespace,
            output,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::FunctionCallOutput {
            id: id.clone(),
            name: name.clone(),
            namespace: namespace.clone(),
            body_kind: tool_output_body_kind(&output.body),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
        },
        ResponseItem::CustomToolCallOutput {
            id,
            name,
            output,
            internal_chat_message_metadata_passthrough,
            ..
        } => ResponsesDecoration::CustomToolCallOutput {
            id: id.clone(),
            name: name.clone(),
            body_kind: tool_output_body_kind(&output.body),
            passthrough: internal_chat_message_metadata_passthrough.clone(),
        },
        _ => return None,
    };
    Some((semantic, decoration))
}

fn tool_output_body_kind(body: &FunctionCallOutputBody) -> ToolOutputBodyKind {
    match body {
        FunctionCallOutputBody::Text(_) => ToolOutputBodyKind::Text,
        FunctionCallOutputBody::ContentItems(_) => ToolOutputBodyKind::ContentItems,
    }
}

fn reconstruct_response_item(item: &HistoryItem, decoration: &ResponsesDecoration) -> ResponseItem {
    match (item, decoration) {
        (
            HistoryItem::Message(message),
            ResponsesDecoration::Message {
                id,
                passthrough,
                text_kinds,
            },
        ) => {
            let mut text_kinds = text_kinds.iter();
            let content = message
                .content
                .iter()
                .map(|part| match part {
                    HistoryMessageContent::Text(text) => match text_kinds.next() {
                        Some(TextContentKind::Input) => {
                            ContentItem::InputText { text: text.clone() }
                        }
                        Some(TextContentKind::Output) => {
                            ContentItem::OutputText { text: text.clone() }
                        }
                        None => unreachable!("message text decoration matches canonical text"),
                    },
                    HistoryMessageContent::Image { source, detail } => ContentItem::InputImage {
                        image_url: uri_source(source),
                        detail: detail.map(image_detail),
                    },
                    HistoryMessageContent::Audio { source } => ContentItem::InputAudio {
                        audio_url: uri_source(source),
                    },
                })
                .collect();
            assert!(
                text_kinds.next().is_none(),
                "message text decoration is exhaustive"
            );
            ResponseItem::Message {
                id: id.clone(),
                role: match message.role {
                    codex_history::HistoryMessageRole::System => "system",
                    codex_history::HistoryMessageRole::Developer => "developer",
                    codex_history::HistoryMessageRole::User => "user",
                    codex_history::HistoryMessageRole::Assistant => "assistant",
                }
                .to_string(),
                content,
                phase: message.phase.map(|phase| match phase {
                    HistoryMessagePhase::Commentary => MessagePhase::Commentary,
                    HistoryMessagePhase::Final => MessagePhase::FinalAnswer,
                }),
                internal_chat_message_metadata_passthrough: passthrough.clone(),
            }
        }
        (
            HistoryItem::Reasoning(reasoning),
            ResponsesDecoration::Reasoning {
                id,
                passthrough,
                content_kinds,
            },
        ) => ResponseItem::Reasoning {
            id: id.clone(),
            summary: reasoning
                .summary
                .iter()
                .map(|text| ReasoningItemReasoningSummary::SummaryText { text: text.clone() })
                .collect(),
            content: content_kinds.as_ref().map(|kinds| {
                assert_eq!(kinds.len(), reasoning.content.len());
                kinds
                    .iter()
                    .zip(&reasoning.content)
                    .map(|(kind, text)| match kind {
                        ReasoningContentKind::ReasoningText => {
                            ReasoningItemContent::ReasoningText { text: text.clone() }
                        }
                        ReasoningContentKind::Text => {
                            ReasoningItemContent::Text { text: text.clone() }
                        }
                    })
                    .collect()
            }),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: passthrough.clone(),
        },
        (HistoryItem::ToolCall(call), ResponsesDecoration::FunctionCall { id, passthrough }) => {
            let codex_history::HistoryToolInput::Json(arguments) = &call.input else {
                unreachable!("function call canonical input is JSON")
            };
            ResponseItem::FunctionCall {
                id: id.clone(),
                name: call.name.clone(),
                namespace: call.namespace.clone(),
                arguments: serde_json::to_string(arguments)
                    .expect("JSON values serialize to function arguments"),
                encrypted_function_args: None,
                call_id: call.call_id.0.clone(),
                internal_chat_message_metadata_passthrough: passthrough.clone(),
            }
        }
        (
            HistoryItem::ToolCall(call),
            ResponsesDecoration::CustomToolCall {
                id,
                status,
                passthrough,
            },
        ) => {
            let codex_history::HistoryToolInput::Text(input) = &call.input else {
                unreachable!("custom tool call canonical input is text")
            };
            ResponseItem::CustomToolCall {
                id: id.clone(),
                status: status.clone(),
                call_id: call.call_id.0.clone(),
                name: call.name.clone(),
                namespace: call.namespace.clone(),
                input: input.clone(),
                internal_chat_message_metadata_passthrough: passthrough.clone(),
            }
        }
        (
            HistoryItem::ToolResult(result),
            ResponsesDecoration::FunctionCallOutput {
                id,
                name,
                namespace,
                body_kind,
                passthrough,
            },
        ) => ResponseItem::FunctionCallOutput {
            id: id.clone(),
            call_id: Some(result.call_id.0.clone()),
            name: name.clone(),
            namespace: namespace.clone(),
            output: tool_output(result, *body_kind),
            internal_chat_message_metadata_passthrough: passthrough.clone(),
        },
        (
            HistoryItem::ToolResult(result),
            ResponsesDecoration::CustomToolCallOutput {
                id,
                name,
                body_kind,
                passthrough,
            },
        ) => ResponseItem::CustomToolCallOutput {
            id: id.clone(),
            call_id: result.call_id.0.clone(),
            name: name.clone(),
            output: tool_output(result, *body_kind),
            internal_chat_message_metadata_passthrough: passthrough.clone(),
        },
        _ => unreachable!("private canonical payload and decoration remain paired"),
    }
}

fn uri_source(source: &codex_history::HistoryMediaSource) -> String {
    match source {
        codex_history::HistoryMediaSource::Uri(uri) => uri.clone(),
        codex_history::HistoryMediaSource::Bytes { .. } => {
            unreachable!("Responses URI projection never stores inline bytes")
        }
    }
}

fn image_detail(detail: codex_history::HistoryImageDetail) -> codex_protocol::models::ImageDetail {
    match detail {
        codex_history::HistoryImageDetail::Auto => codex_protocol::models::ImageDetail::Auto,
        codex_history::HistoryImageDetail::Low => codex_protocol::models::ImageDetail::Low,
        codex_history::HistoryImageDetail::High => codex_protocol::models::ImageDetail::High,
        codex_history::HistoryImageDetail::Original => {
            codex_protocol::models::ImageDetail::Original
        }
    }
}

fn tool_output(
    result: &codex_history::HistoryToolResult,
    body_kind: ToolOutputBodyKind,
) -> FunctionCallOutputPayload {
    let body = match body_kind {
        ToolOutputBodyKind::Text => {
            let [HistoryToolResultContent::Text(text)] = result.content.as_slice() else {
                unreachable!("text output decoration matches one text semantic part")
            };
            FunctionCallOutputBody::Text(text.clone())
        }
        ToolOutputBodyKind::ContentItems => FunctionCallOutputBody::ContentItems(
            result
                .content
                .iter()
                .map(|part| match part {
                    HistoryToolResultContent::Text(text) => {
                        FunctionCallOutputContentItem::InputText { text: text.clone() }
                    }
                    HistoryToolResultContent::Image { source, detail } => {
                        FunctionCallOutputContentItem::InputImage {
                            image_url: uri_source(source),
                            detail: detail.map(image_detail),
                        }
                    }
                    HistoryToolResultContent::Audio { source } => {
                        FunctionCallOutputContentItem::InputAudio {
                            audio_url: uri_source(source),
                        }
                    }
                    HistoryToolResultContent::Json(_)
                    | HistoryToolResultContent::DiscoveredTools(_) => {
                        unreachable!("Responses output projection contains only supported content")
                    }
                })
                .collect(),
        ),
    };
    FunctionCallOutputPayload {
        body,
        success: result.is_error.map(|is_error| !is_error),
    }
}
