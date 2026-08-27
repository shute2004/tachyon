use super::*;

#[test]
fn default_request_is_provider_neutral_and_empty() {
    let request = ModelRequest::default();

    assert!(request.instructions.is_empty());
    assert!(request.input.is_empty());
    assert!(request.tools.is_empty());
    assert!(!request.parallel_tool_calls);
    assert_eq!(request.output.format, ModelOutputFormat::Text);
}

#[test]
fn tool_calls_distinguish_structured_and_freeform_input() {
    let structured = ModelToolCall {
        call_id: ModelToolCallId("call-1".to_string()),
        namespace: Some("workspace".to_string()),
        name: "read_file".to_string(),
        input: ModelToolInput::Json(serde_json::json!({"path": "README.md"})),
    };
    let freeform = ModelToolCall {
        call_id: ModelToolCallId("call-2".to_string()),
        namespace: None,
        name: "shell".to_string(),
        input: ModelToolInput::Text("git status --short".to_string()),
    };

    assert!(matches!(structured.input, ModelToolInput::Json(_)));
    assert!(matches!(freeform.input, ModelToolInput::Text(_)));
}

#[test]
fn freeform_tool_can_preserve_grammar_constraint_without_wire_format() {
    let tool = ModelToolSpec::Freeform {
        namespace: None,
        name: "apply_patch".to_string(),
        description: "Apply a patch".to_string(),
        input_format: ModelFreeformInputFormat::Grammar {
            syntax: "lark".to_string(),
            definition: "start: patch".to_string(),
        },
        availability: ModelToolAvailability::Immediate,
        purpose: ModelToolPurpose::Invocation,
    };

    assert!(matches!(
        tool,
        ModelToolSpec::Freeform {
            input_format: ModelFreeformInputFormat::Grammar { .. },
            ..
        }
    ));
}

#[test]
fn deferred_discovery_semantics_do_not_require_tool_search_wire_type() {
    let deferred_tool = ModelToolSpec::Function {
        namespace: Some("mcp".to_string()),
        name: "expensive_tool".to_string(),
        description: "Loaded after discovery".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        strict: false,
        availability: ModelToolAvailability::Deferred,
        purpose: ModelToolPurpose::Invocation,
    };
    let discovery_tool = ModelToolSpec::Function {
        namespace: None,
        name: "discover_tools".to_string(),
        description: "Discover additional tools".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        strict: false,
        availability: ModelToolAvailability::Immediate,
        purpose: ModelToolPurpose::Discovery,
    };

    assert!(matches!(
        deferred_tool,
        ModelToolSpec::Function {
            availability: ModelToolAvailability::Deferred,
            purpose: ModelToolPurpose::Invocation,
            ..
        }
    ));
    assert!(matches!(
        discovery_tool,
        ModelToolSpec::Function {
            availability: ModelToolAvailability::Immediate,
            purpose: ModelToolPurpose::Discovery,
            ..
        }
    ));
}

#[test]
fn message_phase_preserves_harness_control_flow_semantics() {
    let message = ModelOutputItem::Message {
        id: ModelItemId("message-1".to_string()),
        phase: Some(ModelMessagePhase::Commentary),
        content: vec![ModelContent::Text("working".to_string())],
    };

    assert!(matches!(
        message,
        ModelOutputItem::Message {
            phase: Some(ModelMessagePhase::Commentary),
            ..
        }
    ));
}

#[test]
fn tool_call_start_does_not_require_complete_json_input() {
    let start = ModelOutputItemStart::ToolCall {
        id: ModelItemId("item-1".to_string()),
        call_id: ModelToolCallId("call-1".to_string()),
        namespace: Some("workspace".to_string()),
        name: "read_file".to_string(),
        input_kind: ModelToolInputKind::Json,
    };

    assert!(matches!(
        start,
        ModelOutputItemStart::ToolCall {
            input_kind: ModelToolInputKind::Json,
            ..
        }
    ));
}

#[test]
fn tool_results_keep_structured_output_out_of_message_content() {
    let result = ModelToolResult {
        call_id: ModelToolCallId("call-1".to_string()),
        content: vec![ModelToolResultContent::Json(serde_json::json!({
            "path": "README.md",
            "exists": true,
        }))],
        is_error: false,
    };

    assert!(matches!(
        result.content.as_slice(),
        [ModelToolResultContent::Json(_)]
    ));
}

#[test]
fn completed_reasoning_preserves_plaintext_without_provider_continuation_state() {
    let reasoning = ModelOutputItem::Reasoning {
        id: ModelItemId("reasoning-1".to_string()),
        summary: vec!["summary".to_string()],
        content: vec!["plain reasoning".to_string()],
    };

    let ModelOutputItem::Reasoning {
        summary, content, ..
    } = reasoning
    else {
        panic!("expected reasoning item");
    };
    assert_eq!(summary, vec!["summary"]);
    assert_eq!(content, vec!["plain reasoning"]);
}

#[test]
fn reasoning_section_start_is_generic_stream_structure() {
    let event = ModelEvent::ReasoningSectionStarted {
        item_id: ModelItemId("reasoning-1".to_string()),
        kind: ModelReasoningDeltaKind::Summary,
        section_index: 2,
    };

    assert!(matches!(
        event,
        ModelEvent::ReasoningSectionStarted {
            kind: ModelReasoningDeltaKind::Summary,
            section_index: 2,
            ..
        }
    ));
}

#[test]
fn completion_carries_usage_without_provider_response_identity() {
    let completion = ModelCompletion {
        usage: Some(ModelUsage {
            input_tokens: 100,
            output_tokens: 20,
            cached_input_tokens: Some(60),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(5),
            total_tokens: Some(120),
        }),
        end_turn: Some(true),
    };

    assert_eq!(
        completion.usage.as_ref().map(|usage| usage.output_tokens),
        Some(20)
    );
    assert_eq!(
        completion
            .usage
            .as_ref()
            .and_then(|usage| usage.cache_write_input_tokens),
        None
    );
    assert_eq!(
        completion
            .usage
            .as_ref()
            .and_then(|usage| usage.total_tokens),
        Some(120)
    );
    assert_eq!(completion.end_turn, Some(true));
}

#[test]
fn media_source_can_preserve_bytes_without_assuming_provider_encoding() {
    let bytes = Arc::<[u8]>::from([1_u8, 2, 3]);
    let content = ModelContent::Image {
        source: ModelMediaSource::Bytes {
            media_type: "image/png".to_string(),
            data: Arc::clone(&bytes),
        },
        detail: Some(ModelImageDetail::High),
    };

    let ModelContent::Image {
        source: ModelMediaSource::Bytes { media_type, data },
        detail,
    } = content
    else {
        panic!("expected byte-backed image content");
    };
    assert_eq!(media_type, "image/png");
    assert_eq!(&*data, &[1, 2, 3]);
    assert_eq!(detail, Some(ModelImageDetail::High));
}
