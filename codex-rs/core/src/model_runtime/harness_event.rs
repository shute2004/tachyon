//! Transitional bridge from canonical model events into the current sampling history/tool handlers.
//!
//! Codex-provided `ResponseItem`s and token usage pass through unchanged. Canonical events are
//! projected into the existing internal `ResponseItem` representation without manufacturing
//! Codex event context or compatibility events.

use crate::model_runtime::CodexModelEventContext;
use crate::model_runtime::CodexModelRuntimeSideEvent;
use crate::model_runtime::ModelRuntimeEvent;
use crate::model_runtime::ir::ModelContent;
use crate::model_runtime::ir::ModelEvent;
use crate::model_runtime::ir::ModelImageDetail;
use crate::model_runtime::ir::ModelMessagePhase;
use crate::model_runtime::ir::ModelOutputItem;
use crate::model_runtime::ir::ModelOutputItemStart;
use crate::model_runtime::ir::ModelReasoningDeltaKind;
use crate::model_runtime::ir::ModelToolInput;
use crate::model_runtime::ir::ModelToolInputKind;
use crate::model_runtime::ir::ModelUsage;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;

/// Event shape consumed by the current sampling history and tool handlers.
#[derive(Debug)]
pub(crate) enum HarnessSamplingEvent {
    ItemStarted(ResponseItem),
    ItemCompleted(ResponseItem),
    Completed {
        response_id: Option<String>,
        token_usage: Option<TokenUsage>,
        end_turn: Option<bool>,
    },
    Other(ModelRuntimeEvent),
}

/// Normalizes lifecycle events while leaving all other runtime events on their existing paths.
pub(crate) fn normalize_sampling_event(
    event: ModelRuntimeEvent,
) -> Result<HarnessSamplingEvent, String> {
    match event {
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemStarted(_),
            codex: Some(CodexModelEventContext::OutputItemAdded(item)),
        }
        | ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputItemAdded(item)) => {
            Ok(HarnessSamplingEvent::ItemStarted(item))
        }
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemStarted(start),
            codex: None,
        } => Ok(HarnessSamplingEvent::ItemStarted(response_item_from_start(
            start,
        ))),
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(_),
            codex: Some(CodexModelEventContext::OutputItemCompleted(item)),
        }
        | ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputItemCompleted(item)) => {
            Ok(HarnessSamplingEvent::ItemCompleted(item))
        }
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(item),
            codex: None,
        } => Ok(HarnessSamplingEvent::ItemCompleted(
            response_item_from_completed(item)?,
        )),
        ModelRuntimeEvent::Model {
            event: ModelEvent::Completed(completion),
            codex:
                Some(CodexModelEventContext::Completed {
                    response_id,
                    token_usage,
                }),
        } => Ok(HarnessSamplingEvent::Completed {
            response_id: Some(response_id),
            token_usage,
            end_turn: completion.end_turn,
        }),
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::Completed {
            response_id,
            token_usage,
            end_turn,
        }) => Ok(HarnessSamplingEvent::Completed {
            response_id: Some(response_id),
            token_usage,
            end_turn,
        }),
        ModelRuntimeEvent::Model {
            event: ModelEvent::Completed(completion),
            codex: None,
        } => Ok(HarnessSamplingEvent::Completed {
            response_id: None,
            token_usage: completion.usage.map(token_usage_from_model).transpose()?,
            end_turn: completion.end_turn,
        }),
        ModelRuntimeEvent::Model {
            event:
                ModelEvent::ReasoningSectionStarted {
                    kind: ModelReasoningDeltaKind::Content,
                    ..
                },
            codex: None,
        } => Err(
            "reasoning content section starts are not supported by the current sampling handler"
                .to_string(),
        ),
        other => Ok(HarnessSamplingEvent::Other(other)),
    }
}

fn response_item_from_start(start: ModelOutputItemStart) -> ResponseItem {
    match start {
        ModelOutputItemStart::Message { id, phase } => ResponseItem::Message {
            id: Some(ResponseItemId::from_server(id.0)),
            role: "assistant".to_string(),
            content: Vec::new(),
            phase: phase.map(response_message_phase),
            internal_chat_message_metadata_passthrough: None,
        },
        ModelOutputItemStart::ToolCall {
            id,
            call_id,
            namespace,
            name,
            input_kind: ModelToolInputKind::Json,
        } => ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server(id.0)),
            name,
            namespace,
            arguments: String::new(),
            encrypted_function_args: None,
            call_id: call_id.0,
            internal_chat_message_metadata_passthrough: None,
        },
        ModelOutputItemStart::ToolCall {
            id,
            call_id,
            namespace,
            name,
            input_kind: ModelToolInputKind::Text,
        } => ResponseItem::CustomToolCall {
            id: Some(ResponseItemId::from_server(id.0)),
            status: None,
            call_id: call_id.0,
            name,
            namespace,
            input: String::new(),
            internal_chat_message_metadata_passthrough: None,
        },
        ModelOutputItemStart::Reasoning { id } => ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(id.0)),
            summary: Vec::new(),
            content: Some(Vec::new()),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        },
    }
}

fn response_item_from_completed(item: ModelOutputItem) -> Result<ResponseItem, String> {
    match item {
        ModelOutputItem::Message { id, phase, content } => Ok(ResponseItem::Message {
            id: Some(ResponseItemId::from_server(id.0)),
            role: "assistant".to_string(),
            content: content
                .into_iter()
                .map(response_content_from_model)
                .collect(),
            phase: phase.map(response_message_phase),
            internal_chat_message_metadata_passthrough: None,
        }),
        ModelOutputItem::ToolCall { id, call } => match call.input {
            ModelToolInput::Json(value) => Ok(ResponseItem::FunctionCall {
                id: Some(ResponseItemId::from_server(id.0)),
                name: call.name,
                namespace: call.namespace,
                arguments: serde_json::to_string(&value)
                    .map_err(|err| format!("failed to encode canonical JSON tool input: {err}"))?,
                encrypted_function_args: None,
                call_id: call.call_id.0,
                internal_chat_message_metadata_passthrough: None,
            }),
            ModelToolInput::Text(input) => Ok(ResponseItem::CustomToolCall {
                id: Some(ResponseItemId::from_server(id.0)),
                status: None,
                call_id: call.call_id.0,
                name: call.name,
                namespace: call.namespace,
                input,
                internal_chat_message_metadata_passthrough: None,
            }),
        },
        ModelOutputItem::Reasoning {
            id,
            summary,
            content,
        } => Ok(ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(id.0)),
            summary: summary
                .into_iter()
                .map(|text| ReasoningItemReasoningSummary::SummaryText { text })
                .collect(),
            content: Some(
                content
                    .into_iter()
                    .map(|text| ReasoningItemContent::ReasoningText { text })
                    .collect(),
            ),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }),
    }
}

fn response_content_from_model(content: ModelContent) -> ContentItem {
    match content {
        ModelContent::Text(text) => ContentItem::OutputText { text },
        ModelContent::Image { source, detail } => ContentItem::InputImage {
            image_url: codex_media_source(source),
            detail: detail.map(response_image_detail),
        },
        ModelContent::Audio { source } => ContentItem::InputAudio {
            audio_url: codex_media_source(source),
        },
    }
}

fn codex_media_source(source: crate::model_runtime::ir::ModelMediaSource) -> String {
    match source {
        crate::model_runtime::ir::ModelMediaSource::Uri(uri) => uri,
        crate::model_runtime::ir::ModelMediaSource::Bytes { media_type, data } => {
            codex_utils_image::data_url_from_bytes(&media_type, data.as_ref())
        }
    }
}

fn response_image_detail(detail: ModelImageDetail) -> ImageDetail {
    match detail {
        ModelImageDetail::Auto => ImageDetail::Auto,
        ModelImageDetail::Low => ImageDetail::Low,
        ModelImageDetail::High => ImageDetail::High,
        ModelImageDetail::Original => ImageDetail::Original,
    }
}

fn response_message_phase(phase: ModelMessagePhase) -> MessagePhase {
    match phase {
        ModelMessagePhase::Commentary => MessagePhase::Commentary,
        ModelMessagePhase::Final => MessagePhase::FinalAnswer,
    }
}

fn token_usage_from_model(usage: ModelUsage) -> Result<TokenUsage, String> {
    let input_tokens = i64::try_from(usage.input_tokens)
        .map_err(|_| "canonical input token count exceeds legacy i64 range".to_string())?;
    let output_tokens = i64::try_from(usage.output_tokens)
        .map_err(|_| "canonical output token count exceeds legacy i64 range".to_string())?;
    let cached_input_tokens = usage
        .cached_input_tokens
        .map(token_count_from_model)
        .transpose()?
        .unwrap_or_default();
    let cache_write_input_tokens = usage
        .cache_write_input_tokens
        .map(token_count_from_model)
        .transpose()?
        .unwrap_or_default();
    let reasoning_output_tokens = usage
        .reasoning_output_tokens
        .map(token_count_from_model)
        .transpose()?
        .unwrap_or_default();
    let total_tokens = match usage.total_tokens {
        Some(total) => token_count_from_model(total)?,
        None => {
            let total = usage
                .input_tokens
                .checked_add(usage.output_tokens)
                .ok_or_else(|| "canonical total token count overflowed u64".to_string())?;
            token_count_from_model(total)?
        }
    };

    Ok(TokenUsage {
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
        codex_rollout_budget_units: None,
    })
}

fn token_count_from_model(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "canonical token count exceeds legacy i64 range".to_string())
}

#[cfg(test)]
#[path = "harness_event_tests.rs"]
mod tests;
