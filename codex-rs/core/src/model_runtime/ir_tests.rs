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
fn completion_carries_usage_without_provider_response_identity() {
    let completion = ModelCompletion {
        usage: Some(ModelUsage {
            input_tokens: 100,
            cached_input_tokens: 60,
            cache_write_input_tokens: 0,
            output_tokens: 20,
            reasoning_output_tokens: 5,
            total_tokens: Some(120),
        }),
        end_turn: Some(true),
    };

    assert_eq!(completion.usage.as_ref().map(|usage| usage.output_tokens), Some(20));
    assert_eq!(completion.usage.as_ref().and_then(|usage| usage.total_tokens), Some(120));
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
