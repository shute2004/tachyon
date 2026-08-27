use super::*;

use codex_protocol::ResponseItemId;
use serde_json::json;

fn item_id(prefix: &str) -> ResponseItemId {
    ResponseItemId::with_suffix(prefix, "event-bridge")
}

fn assistant_message(
    id: Option<ResponseItemId>,
    phase: Option<MessagePhase>,
    text: &str,
) -> ResponseItem {
    ResponseItem::Message {
        id,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn created_maps_to_canonical_started() {
    let mut mapper = CodexEventMapper::default();

    let mapped = mapper.map(ResponseEvent::Created);

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Model {
            event: ModelEvent::Started,
            codex: None,
        }
    ));
}

#[test]
fn assistant_message_lifecycle_preserves_phase_and_runtime_correlation() {
    let mut mapper = CodexEventMapper::default();
    let id = item_id("msg");
    let id_text = id.as_str().to_string();

    let started = mapper.map(ResponseEvent::OutputItemAdded(assistant_message(
        Some(id.clone()),
        Some(MessagePhase::Commentary),
        "seed",
    )));
    assert!(matches!(
        started,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemStarted(ModelOutputItemStart::Message {
                id: ModelItemId(ref mapped_id),
                phase: Some(ModelMessagePhase::Commentary),
            }),
            codex: Some(CodexModelEventContext::OutputItemAdded(_)),
        } if mapped_id == &id_text
    ));

    let delta = mapper.map(ResponseEvent::OutputTextDelta(" more".to_string()));
    assert!(matches!(
        delta,
        ModelRuntimeEvent::Model {
            event: ModelEvent::TextDelta {
                item_id: ModelItemId(ref mapped_id),
                ref delta,
            },
            codex: None,
        } if mapped_id == &id_text && delta == " more"
    ));

    let completed = mapper.map(ResponseEvent::OutputItemDone(assistant_message(
        Some(id),
        Some(MessagePhase::FinalAnswer),
        "done",
    )));
    assert!(matches!(
        completed,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
                phase: Some(ModelMessagePhase::Final),
                ..
            }),
            codex: Some(CodexModelEventContext::OutputItemCompleted(_)),
        }
    ));
}

#[test]
fn function_tool_call_uses_partial_start_and_complete_json_only_at_done() {
    let mut mapper = CodexEventMapper::default();
    let id = item_id("fc");
    let id_text = id.as_str().to_string();

    let started = mapper.map(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall {
        id: Some(id.clone()),
        name: "read_file".to_string(),
        namespace: Some("workspace".to_string()),
        arguments: "{".to_string(),
        encrypted_function_args: None,
        call_id: "call-1".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }));
    assert!(matches!(
        started,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemStarted(ModelOutputItemStart::ToolCall {
                input_kind: ModelToolInputKind::Json,
                ..
            }),
            ..
        }
    ));

    let delta = mapper.map(ResponseEvent::ToolCallInputDelta {
        item_id: id_text,
        call_id: Some("call-1".to_string()),
        delta: "\"path\":\"README.md\"}".to_string(),
    });
    assert!(matches!(
        delta,
        ModelRuntimeEvent::Model {
            event: ModelEvent::ToolCallInputDelta {
                call_id: ModelToolCallId(ref call_id),
                ..
            },
            ..
        } if call_id == "call-1"
    ));

    let completed = mapper.map(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
        id: Some(id),
        name: "read_file".to_string(),
        namespace: Some("workspace".to_string()),
        arguments: "{\"path\":\"README.md\"}".to_string(),
        encrypted_function_args: Some(vec!["opaque".to_string()]),
        call_id: "call-1".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }));
    assert!(matches!(
        completed,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(ModelOutputItem::ToolCall {
                call: ModelToolCall {
                    input: ModelToolInput::Json(ref value),
                    ..
                },
                ..
            }),
            codex: Some(CodexModelEventContext::OutputItemCompleted(_)),
        } if value == &json!({"path": "README.md"})
    ));
}

#[test]
fn client_tool_search_call_maps_to_generic_tool_call() {
    let mut mapper = CodexEventMapper::default();

    let mapped = mapper.map(ResponseEvent::OutputItemDone(
        ResponseItem::ToolSearchCall {
            id: Some(item_id("tsc")),
            call_id: Some("search-1".to_string()),
            status: Some("completed".to_string()),
            execution: TOOL_SEARCH_CLIENT_EXECUTION.to_string(),
            arguments: json!({"query": "filesystem"}),
            internal_chat_message_metadata_passthrough: None,
        },
    ));

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(ModelOutputItem::ToolCall {
                call: ModelToolCall {
                    name,
                    input: ModelToolInput::Json(ref input),
                    ..
                },
                ..
            }),
            ..
        } if name == TOOL_SEARCH_NAME && input == &json!({"query": "filesystem"})
    ));
}

#[test]
fn reasoning_completion_preserves_summary_and_plaintext_but_not_encrypted_state() {
    let mut mapper = CodexEventMapper::default();
    let id = item_id("rs");

    let _ = mapper.map(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
        id: Some(id.clone()),
        summary: Vec::new(),
        content: None,
        encrypted_content: Some("opaque-start".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }));

    let mapped = mapper.map(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
        id: Some(id),
        summary: vec![ReasoningItemReasoningSummary::SummaryText {
            text: "summary".to_string(),
        }],
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: "plain reasoning".to_string(),
        }]),
        encrypted_content: Some("opaque-continuation".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }));

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Model {
            event: ModelEvent::OutputItemCompleted(ModelOutputItem::Reasoning {
                ref summary,
                ref content,
                ..
            }),
            codex: Some(CodexModelEventContext::OutputItemCompleted(_)),
        } if summary == &["summary".to_string()] && content == &["plain reasoning".to_string()]
    ));
}

#[test]
fn reasoning_section_start_maps_from_summary_part_structure() {
    let mut mapper = CodexEventMapper::default();
    let id = item_id("rs");
    let id_text = id.as_str().to_string();

    let _ = mapper.map(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
        id: Some(id),
        summary: Vec::new(),
        content: None,
        encrypted_content: None,
        internal_chat_message_metadata_passthrough: None,
    }));

    let mapped = mapper.map(ResponseEvent::ReasoningSummaryPartAdded { summary_index: 2 });
    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Model {
            event: ModelEvent::ReasoningSectionStarted {
                item_id: ModelItemId(ref mapped_id),
                kind: ModelReasoningDeltaKind::Summary,
                section_index: 2,
            },
            ..
        } if mapped_id == &id_text
    ));
}

#[test]
fn completion_maps_generic_usage_while_codex_context_keeps_provider_identity() {
    let mut mapper = CodexEventMapper::default();
    let usage = TokenUsage {
        input_tokens: 100,
        cached_input_tokens: 60,
        cache_write_input_tokens: 10,
        output_tokens: 20,
        reasoning_output_tokens: 5,
        total_tokens: 120,
        codex_rollout_budget_units: Some(serde_json::Number::from(7)),
    };

    let mapped = mapper.map(ResponseEvent::Completed {
        response_id: "resp-provider-private".to_string(),
        token_usage: Some(usage),
        end_turn: Some(true),
    });

    match mapped {
        ModelRuntimeEvent::Model {
            event: ModelEvent::Completed(completion),
            codex:
                Some(CodexModelEventContext::Completed {
                    response_id,
                    token_usage,
                }),
        } => {
            assert_eq!(response_id, "resp-provider-private");
            assert_eq!(completion.end_turn, Some(true));
            let generic = completion.usage.expect("generic usage");
            assert_eq!(generic.input_tokens, 100);
            assert_eq!(generic.cached_input_tokens, Some(60));
            assert_eq!(generic.reasoning_output_tokens, Some(5));
            assert_eq!(
                token_usage.and_then(|usage| usage.codex_rollout_budget_units),
                Some(serde_json::Number::from(7))
            );
        }
        other => panic!("unexpected mapped completion: {other:?}"),
    }
}

#[test]
fn negative_provider_usage_stays_on_compatibility_path() {
    let mut mapper = CodexEventMapper::default();
    let usage = TokenUsage {
        input_tokens: -1,
        ..Default::default()
    };

    let mapped = mapper.map(ResponseEvent::Completed {
        response_id: "resp-negative".to_string(),
        token_usage: Some(usage),
        end_turn: None,
    });

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::Completed { .. })
    ));
}

#[test]
fn product_notification_stays_off_canonical_event_stream() {
    let mut mapper = CodexEventMapper::default();

    let mapped = mapper.map(ResponseEvent::ServerModel("routed-model".to_string()));

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::ServerModel(model))
            if model == "routed-model"
    ));
}

#[test]
fn unsupported_or_uncorrelated_output_start_stays_on_compatibility_path() {
    let mut mapper = CodexEventMapper::default();

    let unsupported = mapper.map(ResponseEvent::OutputItemAdded(ResponseItem::Other));
    assert!(matches!(
        unsupported,
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputItemAdded(
            ResponseItem::Other
        ))
    ));

    let missing_id = mapper.map(ResponseEvent::OutputItemAdded(assistant_message(
        None,
        None,
        "no provider id",
    )));
    assert!(matches!(
        missing_id,
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::OutputItemAdded(_))
    ));
}

#[test]
fn sequential_reasoning_summary_done_remains_compatibility_until_normalized() {
    let mut mapper = CodexEventMapper::default();

    let mapped = mapper.map(ResponseEvent::ReasoningSummaryDone {
        item_id: "rs_1".to_string(),
        text: "whole section".to_string(),
        summary_index: 1,
    });

    assert!(matches!(
        mapped,
        ModelRuntimeEvent::Compatibility(CodexModelRuntimeSideEvent::ReasoningSummaryDone {
            summary_index: 1,
            ..
        })
    ));
}
