use std::sync::Arc;

use super::HarnessSamplingEvent;
use super::normalize_sampling_event;
use crate::model_runtime::CodexModelEventContext;
use crate::model_runtime::CodexModelRuntimeSideEvent;
use crate::model_runtime::ModelRuntimeEvent;
use crate::model_runtime::ir::ModelCompletion;
use crate::model_runtime::ir::ModelContent;
use crate::model_runtime::ir::ModelEvent;
use crate::model_runtime::ir::ModelImageDetail;
use crate::model_runtime::ir::ModelItemId;
use crate::model_runtime::ir::ModelMediaSource;
use crate::model_runtime::ir::ModelMessagePhase;
use crate::model_runtime::ir::ModelOutputItem;
use crate::model_runtime::ir::ModelOutputItemStart;
use crate::model_runtime::ir::ModelReasoningDeltaKind;
use crate::model_runtime::ir::ModelToolCall;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolInput;
use crate::model_runtime::ir::ModelToolInputKind;
use crate::model_runtime::ir::ModelUsage;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;

fn canonical_event(event: ModelEvent) -> ModelRuntimeEvent {
    ModelRuntimeEvent::Model { event, codex: None }
}

fn item_started(event: ModelEvent) -> ResponseItem {
    match normalize_sampling_event(canonical_event(event)).unwrap() {
        HarnessSamplingEvent::ItemStarted(item) => item,
        event => panic!("expected a started item, got {event:?}"),
    }
}

fn item_completed(event: ModelEvent) -> ResponseItem {
    match normalize_sampling_event(canonical_event(event)).unwrap() {
        HarnessSamplingEvent::ItemCompleted(item) => item,
        event => panic!("expected a completed item, got {event:?}"),
    }
}

fn completed(usage: Option<ModelUsage>, end_turn: Option<bool>) -> HarnessSamplingEvent {
    normalize_sampling_event(canonical_event(ModelEvent::Completed(ModelCompletion {
        usage,
        end_turn,
    })))
    .unwrap()
}

#[test]
fn canonical_starts_preserve_ids_phase_namespace_and_empty_partial_inputs() {
    assert_eq!(
        item_started(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::Message {
                id: ModelItemId("msg_start".to_string()),
                phase: Some(ModelMessagePhase::Commentary),
            },
        )),
        ResponseItem::Message {
            id: Some(ResponseItemId::from_server("msg_start".to_string())),
            role: "assistant".to_string(),
            content: Vec::new(),
            phase: Some(MessagePhase::Commentary),
            internal_chat_message_metadata_passthrough: None,
        }
    );

    assert_eq!(
        item_started(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::ToolCall {
                id: ModelItemId("fc_start".to_string()),
                call_id: ModelToolCallId("call-json".to_string()),
                namespace: Some("workspace".to_string()),
                name: "read_file".to_string(),
                input_kind: ModelToolInputKind::Json,
            },
        )),
        ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server("fc_start".to_string())),
            name: "read_file".to_string(),
            namespace: Some("workspace".to_string()),
            arguments: String::new(),
            encrypted_function_args: None,
            call_id: "call-json".to_string(),
            internal_chat_message_metadata_passthrough: None,
        }
    );

    assert_eq!(
        item_started(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::ToolCall {
                id: ModelItemId("ctc_start".to_string()),
                call_id: ModelToolCallId("call-text".to_string()),
                namespace: Some("shell".to_string()),
                name: "run".to_string(),
                input_kind: ModelToolInputKind::Text,
            },
        )),
        ResponseItem::CustomToolCall {
            id: Some(ResponseItemId::from_server("ctc_start".to_string())),
            status: None,
            call_id: "call-text".to_string(),
            name: "run".to_string(),
            namespace: Some("shell".to_string()),
            input: String::new(),
            internal_chat_message_metadata_passthrough: None,
        }
    );

    assert_eq!(
        item_started(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::Reasoning {
                id: ModelItemId("rs_start".to_string()),
            },
        )),
        ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server("rs_start".to_string())),
            summary: Vec::new(),
            content: Some(Vec::new()),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }
    );
}

#[test]
fn canonical_completed_messages_preserve_text_phase_and_media_payloads() {
    let item = item_completed(ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
        id: ModelItemId("msg_done".to_string()),
        phase: Some(ModelMessagePhase::Final),
        content: vec![
            ModelContent::Text("answer".to_string()),
            ModelContent::Image {
                source: ModelMediaSource::Uri("https://example.test/image.png".to_string()),
                detail: Some(ModelImageDetail::High),
            },
            ModelContent::Image {
                source: ModelMediaSource::Bytes {
                    media_type: "image/png".to_string(),
                    data: Arc::<[u8]>::from(vec![0, 1, 2]),
                },
                detail: Some(ModelImageDetail::Original),
            },
            ModelContent::Audio {
                source: ModelMediaSource::Uri("https://example.test/audio.wav".to_string()),
            },
            ModelContent::Audio {
                source: ModelMediaSource::Bytes {
                    media_type: "audio/wav".to_string(),
                    data: Arc::<[u8]>::from(vec![3, 4]),
                },
            },
        ],
    }));

    assert_eq!(
        item,
        ResponseItem::Message {
            id: Some(ResponseItemId::from_server("msg_done".to_string())),
            role: "assistant".to_string(),
            content: vec![
                ContentItem::OutputText {
                    text: "answer".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "https://example.test/image.png".to_string(),
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,AAEC".to_string(),
                    detail: Some(ImageDetail::Original),
                },
                ContentItem::InputAudio {
                    audio_url: "https://example.test/audio.wav".to_string(),
                },
                ContentItem::InputAudio {
                    audio_url: "data:audio/wav;base64,AwQ=".to_string(),
                },
            ],
            phase: Some(MessagePhase::FinalAnswer),
            internal_chat_message_metadata_passthrough: None,
        }
    );
}

#[test]
fn canonical_completed_tool_calls_preserve_json_text_and_namespace() {
    assert_eq!(
        item_completed(ModelEvent::OutputItemCompleted(ModelOutputItem::ToolCall {
            id: ModelItemId("fc_done".to_string()),
            call: ModelToolCall {
                call_id: ModelToolCallId("call-json".to_string()),
                namespace: Some("workspace".to_string()),
                name: "read_file".to_string(),
                input: ModelToolInput::Json(serde_json::json!({"path": "README.md"})),
            },
        },)),
        ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server("fc_done".to_string())),
            name: "read_file".to_string(),
            namespace: Some("workspace".to_string()),
            arguments: r#"{"path":"README.md"}"#.to_string(),
            encrypted_function_args: None,
            call_id: "call-json".to_string(),
            internal_chat_message_metadata_passthrough: None,
        }
    );

    assert_eq!(
        item_completed(ModelEvent::OutputItemCompleted(ModelOutputItem::ToolCall {
            id: ModelItemId("ctc_done".to_string()),
            call: ModelToolCall {
                call_id: ModelToolCallId("call-text".to_string()),
                namespace: Some("shell".to_string()),
                name: "run".to_string(),
                input: ModelToolInput::Text("echo hello".to_string()),
            },
        },)),
        ResponseItem::CustomToolCall {
            id: Some(ResponseItemId::from_server("ctc_done".to_string())),
            status: None,
            call_id: "call-text".to_string(),
            name: "run".to_string(),
            namespace: Some("shell".to_string()),
            input: "echo hello".to_string(),
            internal_chat_message_metadata_passthrough: None,
        }
    );
}

#[test]
fn canonical_completed_reasoning_preserves_summary_and_content() {
    assert_eq!(
        item_completed(ModelEvent::OutputItemCompleted(
            ModelOutputItem::Reasoning {
                id: ModelItemId("rs_done".to_string()),
                summary: vec!["summary".to_string()],
                content: vec!["reasoning".to_string()],
            },
        )),
        ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server("rs_done".to_string())),
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "summary".to_string(),
            }],
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: "reasoning".to_string(),
            }]),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }
    );
}

#[test]
fn canonical_completion_preserves_absent_zero_and_unknown_usage() {
    assert!(matches!(
        completed(None, Some(false)),
        HarnessSamplingEvent::Completed {
            response_id: None,
            token_usage: None,
            end_turn: Some(false),
        }
    ));

    assert!(matches!(
        completed(Some(ModelUsage::default()), None),
        HarnessSamplingEvent::Completed {
            response_id: None,
            token_usage: Some(TokenUsage {
                input_tokens: 0,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 0,
                codex_rollout_budget_units: None,
            }),
            end_turn: None,
        }
    ));

    assert!(matches!(
        completed(
            Some(ModelUsage {
                input_tokens: 2,
                output_tokens: 3,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: None,
            }),
            None,
        ),
        HarnessSamplingEvent::Completed {
            response_id: None,
            token_usage: Some(TokenUsage {
                input_tokens: 2,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 3,
                reasoning_output_tokens: 0,
                total_tokens: 5,
                codex_rollout_budget_units: None,
            }),
            end_turn: None,
        }
    ));
}

#[test]
fn canonical_completion_preserves_reported_total_and_rejects_overflow() {
    assert!(matches!(
        completed(
            Some(ModelUsage {
                input_tokens: 2,
                output_tokens: 3,
                cached_input_tokens: Some(1),
                cache_write_input_tokens: Some(4),
                reasoning_output_tokens: Some(2),
                total_tokens: Some(19),
            }),
            None,
        ),
        HarnessSamplingEvent::Completed {
            token_usage: Some(TokenUsage {
                input_tokens: 2,
                cached_input_tokens: 1,
                cache_write_input_tokens: 4,
                output_tokens: 3,
                reasoning_output_tokens: 2,
                total_tokens: 19,
                codex_rollout_budget_units: None,
            }),
            ..
        }
    ));

    let input_conversion_overflow =
        normalize_sampling_event(canonical_event(ModelEvent::Completed(ModelCompletion {
            usage: Some(ModelUsage {
                input_tokens: u64::MAX,
                ..ModelUsage::default()
            }),
            end_turn: None,
        })));
    assert!(
        input_conversion_overflow
            .unwrap_err()
            .contains("input token count exceeds legacy i64 range")
    );

    let total_conversion_overflow =
        normalize_sampling_event(canonical_event(ModelEvent::Completed(ModelCompletion {
            usage: Some(ModelUsage {
                input_tokens: i64::MAX as u64,
                output_tokens: 1,
                ..ModelUsage::default()
            }),
            end_turn: None,
        })));
    assert!(
        total_conversion_overflow
            .unwrap_err()
            .contains("token count exceeds legacy i64 range")
    );
}

#[test]
fn codex_item_and_completion_context_pass_through_without_losing_decorations() {
    let legacy_item = ResponseItem::Message {
        id: Some(ResponseItemId::from_server("msg_codex".to_string())),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: "legacy".to_string(),
        }],
        phase: Some(MessagePhase::FinalAnswer),
        internal_chat_message_metadata_passthrough: Some(InternalChatMessageMetadataPassthrough {
            turn_id: Some("turn-codex".to_string()),
            ..InternalChatMessageMetadataPassthrough::default()
        }),
    };
    let normalized = normalize_sampling_event(ModelRuntimeEvent::Model {
        event: ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
            id: ModelItemId("msg_codex".to_string()),
            phase: Some(ModelMessagePhase::Final),
            content: vec![ModelContent::Text("legacy".to_string())],
        }),
        codex: Some(CodexModelEventContext::OutputItemCompleted(
            legacy_item.clone(),
        )),
    })
    .unwrap();
    assert!(matches!(
        normalized,
        HarnessSamplingEvent::ItemCompleted(item) if item == legacy_item
    ));

    let token_usage = TokenUsage {
        input_tokens: 11,
        cached_input_tokens: 3,
        cache_write_input_tokens: 2,
        output_tokens: 5,
        reasoning_output_tokens: 4,
        total_tokens: 16,
        codex_rollout_budget_units: Some(serde_json::Number::from(23)),
    };
    let normalized = normalize_sampling_event(ModelRuntimeEvent::Model {
        event: ModelEvent::Completed(ModelCompletion {
            usage: None,
            end_turn: Some(false),
        }),
        codex: Some(CodexModelEventContext::Completed {
            response_id: "resp_codex".to_string(),
            token_usage: Some(token_usage.clone()),
        }),
    })
    .unwrap();
    assert!(matches!(
        normalized,
        HarnessSamplingEvent::Completed {
            response_id: Some(response_id),
            token_usage: Some(actual_usage),
            end_turn: Some(false),
        } if response_id == "resp_codex" && actual_usage == token_usage
    ));

    let compatibility_usage = TokenUsage {
        input_tokens: 7,
        total_tokens: 7,
        ..TokenUsage::default()
    };
    let normalized = normalize_sampling_event(ModelRuntimeEvent::Compatibility(
        CodexModelRuntimeSideEvent::Completed {
            response_id: "resp_compat".to_string(),
            token_usage: Some(compatibility_usage.clone()),
            end_turn: Some(true),
        },
    ))
    .unwrap();
    assert!(matches!(
        normalized,
        HarnessSamplingEvent::Completed {
            response_id: Some(response_id),
            token_usage: Some(actual_usage),
            end_turn: Some(true),
        } if response_id == "resp_compat" && actual_usage == compatibility_usage
    ));
}

#[test]
fn generic_reasoning_content_section_start_fails_explicitly() {
    let error = normalize_sampling_event(canonical_event(ModelEvent::ReasoningSectionStarted {
        item_id: ModelItemId("rs_content".to_string()),
        kind: ModelReasoningDeltaKind::Content,
        section_index: 0,
    }))
    .unwrap_err();

    assert!(error.contains("reasoning content section starts are not supported"));
}

#[test]
fn unrelated_canonical_events_remain_on_the_other_path() {
    assert!(matches!(
        normalize_sampling_event(canonical_event(ModelEvent::Started)).unwrap(),
        HarnessSamplingEvent::Other(ModelRuntimeEvent::Model {
            event: ModelEvent::Started,
            codex: None,
        })
    ));
}
