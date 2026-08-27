//! Transitional conversion from Codex/OpenAI stream events into Tachyon's canonical model events.
//!
//! C3 deliberately maps only stream semantics that the current canonical IR can represent without
//! loss. Product/backend notifications and model-event shapes that still need a generic harness
//! representation remain on an explicit Codex compatibility side channel.

use crate::client_common::ResponseEvent;
use crate::model_runtime::ir::ModelCompletion;
use crate::model_runtime::ir::ModelContent;
use crate::model_runtime::ir::ModelEvent;
use crate::model_runtime::ir::ModelItemId;
use crate::model_runtime::ir::ModelMessagePhase;
use crate::model_runtime::ir::ModelOutputItem;
use crate::model_runtime::ir::ModelOutputItemStart;
use crate::model_runtime::ir::ModelReasoningDeltaKind;
use crate::model_runtime::ir::ModelToolCall;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolInput;
use crate::model_runtime::ir::ModelToolInputKind;
use crate::model_runtime::ir::ModelUsage;
use codex_protocol::models::ContentItem;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ModelVerification;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnModerationMetadataEvent;

const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_CLIENT_EXECUTION: &str = "client";

/// One event exposed by the transitional C3 model-runtime stream boundary.
///
/// `Model` carries provider-neutral model semantics. `Compatibility` is intentionally Codex-only
/// and exists so unsupported model events and product/backend notifications can preserve current
/// behavior without contaminating `ModelEvent`.
#[derive(Debug)]
pub(crate) enum ModelRuntimeEvent {
    Model {
        event: ModelEvent,
        codex: Option<CodexModelEventContext>,
    },
    Compatibility(CodexModelRuntimeSideEvent),
}

/// Migration-only Codex context for generic events whose current harness handlers still require
/// provider-shaped data. These values are not part of the canonical model contract.
#[derive(Debug)]
pub(crate) enum CodexModelEventContext {
    OutputItemAdded(ResponseItem),
    OutputItemCompleted(ResponseItem),
    Completed {
        response_id: String,
        token_usage: Option<TokenUsage>,
    },
}

/// Explicit Codex compatibility side channel used while C3 is incomplete.
///
/// Some variants are backend/product notifications and should never become `ModelEvent`. Others are
/// model events whose generic harness semantics have not yet been extracted. Keeping them explicit
/// prevents an opaque JSON/provider enum from leaking into the canonical IR.
#[derive(Debug)]
pub(crate) enum CodexModelRuntimeSideEvent {
    SafetyBuffering {
        use_cases: Vec<String>,
        reasons: Vec<String>,
        show_buffering_ui: bool,
        faster_model: Option<String>,
    },
    OutputItemAdded(ResponseItem),
    OutputItemCompleted(ResponseItem),
    ServerModel(String),
    ModelVerifications(Vec<ModelVerification>),
    TurnModerationMetadata(TurnModerationMetadataEvent),
    ServerReasoningIncluded(bool),
    Completed {
        response_id: String,
        token_usage: Option<TokenUsage>,
        end_turn: Option<bool>,
    },
    OutputTextDelta(String),
    ToolCallInputDelta {
        #[allow(dead_code)]
        item_id: String,
        call_id: Option<String>,
        delta: String,
    },
    ReasoningSummaryDelta {
        delta: String,
        summary_index: i64,
    },
    ReasoningSummaryDone {
        item_id: String,
        text: String,
        summary_index: i64,
    },
    ReasoningContentDelta {
        delta: String,
        content_index: i64,
    },
    ReasoningSummaryPartAdded {
        summary_index: i64,
    },
    RateLimits(RateLimitSnapshot),
    ModelsEtag(String),
}

#[derive(Debug, Clone)]
enum ActiveCanonicalItem {
    Message {
        item_id: ModelItemId,
    },
    ToolCall {
        item_id: ModelItemId,
        call_id: ModelToolCallId,
    },
    Reasoning {
        item_id: ModelItemId,
    },
}

/// Stateful mapper for one turn-scoped Codex model runtime.
///
/// Stream correlation state is reset by `ResponseEvent::Created`, which occurs for each upstream
/// response, so the same mapper can be reused across tool follow-ups and retries within one harness
/// turn without carrying an active output item across responses.
#[derive(Debug, Default)]
pub(super) struct CodexEventMapper {
    active: Option<ActiveCanonicalItem>,
}

impl CodexEventMapper {
    pub(super) fn map(&mut self, event: ResponseEvent) -> ModelRuntimeEvent {
        match event {
            ResponseEvent::Created => {
                self.active = None;
                model_event(ModelEvent::Started)
            }
            ResponseEvent::OutputItemAdded(item) => self.map_output_item_added(item),
            ResponseEvent::OutputItemDone(item) => self.map_output_item_completed(item),
            ResponseEvent::Completed {
                response_id,
                token_usage,
                end_turn,
            } => {
                self.active = None;
                let usage = match token_usage.as_ref() {
                    Some(usage) => {
                        let Some(usage) = model_usage(usage) else {
                            return ModelRuntimeEvent::Compatibility(
                                CodexModelRuntimeSideEvent::Completed {
                                    response_id,
                                    token_usage,
                                    end_turn,
                                },
                            );
                        };
                        Some(usage)
                    }
                    None => None,
                };
                ModelRuntimeEvent::Model {
                    event: ModelEvent::Completed(ModelCompletion { usage, end_turn }),
                    codex: Some(CodexModelEventContext::Completed {
                        response_id,
                        token_usage,
                    }),
                }
            }
            ResponseEvent::OutputTextDelta(delta) => match self.active.as_ref() {
                Some(ActiveCanonicalItem::Message { item_id }) => {
                    model_event(ModelEvent::TextDelta {
                        item_id: item_id.clone(),
                        delta,
                    })
                }
                _ => ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputTextDelta(
                    delta,
                )),
            },
            ResponseEvent::ToolCallInputDelta {
                item_id,
                call_id,
                delta,
            } => {
                let Some(ActiveCanonicalItem::ToolCall {
                    item_id: active_item_id,
                    call_id: active_call_id,
                }) = self.active.as_ref()
                else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ToolCallInputDelta {
                            item_id,
                            call_id,
                            delta,
                        },
                    );
                };
                if (!item_id.is_empty() && item_id != active_item_id.0)
                    || call_id
                        .as_deref()
                        .is_some_and(|call_id| call_id != active_call_id.0)
                {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ToolCallInputDelta {
                            item_id,
                            call_id,
                            delta,
                        },
                    );
                }
                model_event(ModelEvent::ToolCallInputDelta {
                    item_id: active_item_id.clone(),
                    call_id: active_call_id.clone(),
                    delta,
                })
            }
            ResponseEvent::ReasoningSummaryDelta {
                delta,
                summary_index,
            } => {
                let Some(ActiveCanonicalItem::Reasoning { item_id }) = self.active.as_ref() else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningSummaryDelta {
                            delta,
                            summary_index,
                        },
                    );
                };
                let Ok(section_index) = u32::try_from(summary_index) else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningSummaryDelta {
                            delta,
                            summary_index,
                        },
                    );
                };
                model_event(ModelEvent::ReasoningDelta {
                    item_id: item_id.clone(),
                    kind: ModelReasoningDeltaKind::Summary,
                    delta,
                    section_index: Some(section_index),
                })
            }
            ResponseEvent::ReasoningSummaryPartAdded { summary_index } => {
                let Some(ActiveCanonicalItem::Reasoning { item_id }) = self.active.as_ref() else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningSummaryPartAdded { summary_index },
                    );
                };
                let Ok(section_index) = u32::try_from(summary_index) else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningSummaryPartAdded { summary_index },
                    );
                };
                model_event(ModelEvent::ReasoningSectionStarted {
                    item_id: item_id.clone(),
                    kind: ModelReasoningDeltaKind::Summary,
                    section_index,
                })
            }
            ResponseEvent::ReasoningContentDelta {
                delta,
                content_index,
            } => {
                let Some(ActiveCanonicalItem::Reasoning { item_id }) = self.active.as_ref() else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningContentDelta {
                            delta,
                            content_index,
                        },
                    );
                };
                let Ok(section_index) = u32::try_from(content_index) else {
                    return ModelRuntimeEvent::Compatibility(
                        CodexModelRuntimeSideEvent::ReasoningContentDelta {
                            delta,
                            content_index,
                        },
                    );
                };
                model_event(ModelEvent::ReasoningDelta {
                    item_id: item_id.clone(),
                    kind: ModelReasoningDeltaKind::Content,
                    delta,
                    section_index: Some(section_index),
                })
            }
            // Sequential-cutoff summary delivery has distinct current consumer behavior. Keep it
            // on the compatibility path until C3 normalizes that delivery without changing the
            // section-break semantics.
            ResponseEvent::ReasoningSummaryDone {
                item_id,
                text,
                summary_index,
            } => {
                ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::ReasoningSummaryDone {
                    item_id,
                    text,
                    summary_index,
                })
            }
            ResponseEvent::SafetyBuffering(buffering) => {
                ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::SafetyBuffering {
                    use_cases: buffering.use_cases,
                    reasons: buffering.reasons,
                    show_buffering_ui: buffering.show_buffering_ui,
                    faster_model: buffering.faster_model,
                })
            }
            ResponseEvent::ServerModel(server_model) => ModelRuntimeEvent::Compatibility(
                CodexModelRuntimeSideEvent::ServerModel(server_model),
            ),
            ResponseEvent::ModelVerifications(verifications) => ModelRuntimeEvent::Compatibility(
                CodexModelRuntimeSideEvent::ModelVerifications(verifications),
            ),
            ResponseEvent::TurnModerationMetadata(metadata) => ModelRuntimeEvent::Compatibility(
                CodexModelRuntimeSideEvent::TurnModerationMetadata(metadata),
            ),
            ResponseEvent::ServerReasoningIncluded(included) => ModelRuntimeEvent::Compatibility(
                CodexModelRuntimeSideEvent::ServerReasoningIncluded(included),
            ),
            ResponseEvent::RateLimits(snapshot) => {
                ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::RateLimits(snapshot))
            }
            ResponseEvent::ModelsEtag(etag) => {
                ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::ModelsEtag(etag))
            }
        }
    }

    fn map_output_item_added(&mut self, item: ResponseItem) -> ModelRuntimeEvent {
        let Some((start, active)) = model_output_item_start(&item) else {
            self.active = None;
            return ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputItemAdded(
                item,
            ));
        };
        self.active = Some(active);
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemStarted(start),
            codex: Some(CodexModelEventContext::OutputItemAdded(item)),
        }
    }

    fn map_output_item_completed(&mut self, item: ResponseItem) -> ModelRuntimeEvent {
        let completed = model_output_item_completed(&item, self.active.as_ref());
        self.active = None;
        let Some(completed) = completed else {
            return ModelRuntimeEvent::Compatibility(
                CodexModelRuntimeSideEvent::OutputItemCompleted(item),
            );
        };
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(completed),
            codex: Some(CodexModelEventContext::OutputItemCompleted(item)),
        }
    }
}

fn model_event(event: ModelEvent) -> ModelRuntimeEvent {
    ModelRuntimeEvent::Model { event, codex: None }
}

fn model_output_item_start(
    item: &ResponseItem,
) -> Option<(ModelOutputItemStart, ActiveCanonicalItem)> {
    let item_id = model_item_id(item)?;
    match item {
        ResponseItem::Message { role, phase, .. } if role == "assistant" => {
            let start = ModelOutputItemStart::Message {
                id: item_id.clone(),
                phase: phase.as_ref().map(model_message_phase),
            };
            Some((start, ActiveCanonicalItem::Message { item_id }))
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            call_id,
            ..
        } if !call_id.is_empty() => {
            let call_id = ModelToolCallId(call_id.clone());
            let start = ModelOutputItemStart::ToolCall {
                id: item_id.clone(),
                call_id: call_id.clone(),
                namespace: namespace.clone(),
                name: name.clone(),
                input_kind: ModelToolInputKind::Json,
            };
            Some((start, ActiveCanonicalItem::ToolCall { item_id, call_id }))
        }
        ResponseItem::CustomToolCall {
            call_id,
            name,
            namespace,
            ..
        } if !call_id.is_empty() => {
            let call_id = ModelToolCallId(call_id.clone());
            let start = ModelOutputItemStart::ToolCall {
                id: item_id.clone(),
                call_id: call_id.clone(),
                namespace: namespace.clone(),
                name: name.clone(),
                input_kind: ModelToolInputKind::Text,
            };
            Some((start, ActiveCanonicalItem::ToolCall { item_id, call_id }))
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            execution,
            ..
        } if execution == TOOL_SEARCH_CLIENT_EXECUTION && !call_id.is_empty() => {
            let call_id = ModelToolCallId(call_id.clone());
            let start = ModelOutputItemStart::ToolCall {
                id: item_id.clone(),
                call_id: call_id.clone(),
                namespace: None,
                name: TOOL_SEARCH_NAME.to_string(),
                input_kind: ModelToolInputKind::Json,
            };
            Some((start, ActiveCanonicalItem::ToolCall { item_id, call_id }))
        }
        ResponseItem::Reasoning { .. } => {
            let start = ModelOutputItemStart::Reasoning {
                id: item_id.clone(),
            };
            Some((start, ActiveCanonicalItem::Reasoning { item_id }))
        }
        _ => None,
    }
}

fn model_output_item_completed(
    item: &ResponseItem,
    active: Option<&ActiveCanonicalItem>,
) -> Option<ModelOutputItem> {
    match item {
        ResponseItem::Message {
            role,
            content,
            phase,
            ..
        } if role == "assistant" => {
            let id = completed_item_id(item, active, ActiveKind::Message)?;
            let content = content
                .iter()
                .map(|content| match content {
                    ContentItem::OutputText { text } => Some(ModelContent::Text(text.clone())),
                    ContentItem::InputText { .. }
                    | ContentItem::InputImage { .. }
                    | ContentItem::InputAudio { .. } => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ModelOutputItem::Message {
                id,
                phase: phase.as_ref().map(model_message_phase),
                content,
            })
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } if !call_id.is_empty() => {
            let id = completed_item_id(item, active, ActiveKind::ToolCall)?;
            let input = serde_json::from_str(arguments).ok()?;
            Some(ModelOutputItem::ToolCall {
                id,
                call: ModelToolCall {
                    call_id: ModelToolCallId(call_id.clone()),
                    namespace: namespace.clone(),
                    name: name.clone(),
                    input: ModelToolInput::Json(input),
                },
            })
        }
        ResponseItem::CustomToolCall {
            call_id,
            name,
            namespace,
            input,
            ..
        } if !call_id.is_empty() => {
            let id = completed_item_id(item, active, ActiveKind::ToolCall)?;
            Some(ModelOutputItem::ToolCall {
                id,
                call: ModelToolCall {
                    call_id: ModelToolCallId(call_id.clone()),
                    namespace: namespace.clone(),
                    name: name.clone(),
                    input: ModelToolInput::Text(input.clone()),
                },
            })
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            execution,
            arguments,
            ..
        } if execution == TOOL_SEARCH_CLIENT_EXECUTION && !call_id.is_empty() => {
            let id = completed_item_id(item, active, ActiveKind::ToolCall)?;
            Some(ModelOutputItem::ToolCall {
                id,
                call: ModelToolCall {
                    call_id: ModelToolCallId(call_id.clone()),
                    namespace: None,
                    name: TOOL_SEARCH_NAME.to_string(),
                    input: ModelToolInput::Json(arguments.clone()),
                },
            })
        }
        ResponseItem::Reasoning {
            summary, content, ..
        } => {
            let id = completed_item_id(item, active, ActiveKind::Reasoning)?;
            let summary = summary
                .iter()
                .map(|summary| match summary {
                    ReasoningItemReasoningSummary::SummaryText { text } => text.clone(),
                })
                .collect();
            let content = content
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|content| match content {
                    ReasoningItemContent::ReasoningText { text }
                    | ReasoningItemContent::Text { text } => text.clone(),
                })
                .collect();
            Some(ModelOutputItem::Reasoning {
                id,
                summary,
                content,
            })
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum ActiveKind {
    Message,
    ToolCall,
    Reasoning,
}

fn completed_item_id(
    item: &ResponseItem,
    active: Option<&ActiveCanonicalItem>,
    expected: ActiveKind,
) -> Option<ModelItemId> {
    let explicit = model_item_id(item);
    let active = match (active, expected) {
        (Some(ActiveCanonicalItem::Message { item_id }), ActiveKind::Message)
        | (Some(ActiveCanonicalItem::ToolCall { item_id, .. }), ActiveKind::ToolCall)
        | (Some(ActiveCanonicalItem::Reasoning { item_id }), ActiveKind::Reasoning) => {
            Some(item_id.clone())
        }
        (Some(_), _) => return None,
        (None, _) => None,
    };
    match (explicit, active) {
        (Some(explicit), Some(active)) if explicit != active => None,
        (Some(explicit), _) => Some(explicit),
        (None, Some(active)) => Some(active),
        (None, None) => None,
    }
}

fn model_item_id(item: &ResponseItem) -> Option<ModelItemId> {
    item.id()
        .map(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .map(|id| ModelItemId(id.to_string()))
}

fn model_message_phase(phase: &MessagePhase) -> ModelMessagePhase {
    match phase {
        MessagePhase::Commentary => ModelMessagePhase::Commentary,
        MessagePhase::FinalAnswer => ModelMessagePhase::Final,
    }
}

fn model_usage(usage: &TokenUsage) -> Option<ModelUsage> {
    Some(ModelUsage {
        input_tokens: u64::try_from(usage.input_tokens).ok()?,
        output_tokens: u64::try_from(usage.output_tokens).ok()?,
        cached_input_tokens: Some(u64::try_from(usage.cached_input_tokens).ok()?),
        cache_write_input_tokens: Some(u64::try_from(usage.cache_write_input_tokens).ok()?),
        reasoning_output_tokens: Some(u64::try_from(usage.reasoning_output_tokens).ok()?),
        total_tokens: Some(u64::try_from(usage.total_tokens).ok()?),
    })
}

#[cfg(test)]
#[path = "codex_event_tests.rs"]
mod tests;
