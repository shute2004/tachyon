use std::collections::BTreeSet;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use schemars::schema_for;
use serde_json::Value;
use serde_json::json;

use super::*;

#[test]
fn every_model_input_item_variant_roundtrips_as_a_whole() {
    let tool = ModelToolSpec::Namespace {
        name: "workspace".to_string(),
        description: "Workspace tools".to_string(),
        tools: vec![
            ModelToolSpec::Function {
                namespace: None,
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                output_schema: Some(json!({"type": "string"})),
                strict: true,
                availability: ModelToolAvailability::Deferred,
                purpose: ModelToolPurpose::Discovery,
            },
            ModelToolSpec::Freeform {
                namespace: None,
                name: "apply_patch".to_string(),
                description: "Apply a patch".to_string(),
                input_format: ModelFreeformInputFormat::Grammar {
                    syntax: "lark".to_string(),
                    definition: "start: patch".to_string(),
                },
                availability: ModelToolAvailability::Immediate,
                purpose: ModelToolPurpose::Invocation,
            },
        ],
    };
    let items = vec![
        ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![
                ModelContent::Text("Describe this image".to_string()),
                ModelContent::Image {
                    source: ModelMediaSource::Uri("data:image/png;base64,AA==".to_string()),
                    detail: Some(ModelImageDetail::High),
                },
                ModelContent::Audio {
                    source: ModelMediaSource::Bytes {
                        media_type: "audio/wav".to_string(),
                        data: Arc::from(vec![0, 1, 0xff, 0]),
                    },
                },
            ],
        }),
        ModelInputItem::Reasoning(ModelReasoning {
            summary: vec!["Compare the requested file and image".to_string()],
            content: vec!["The image is attached as user input.".to_string()],
        }),
        ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId("call-1".to_string()),
            namespace: Some("workspace".to_string()),
            name: "read_file".to_string(),
            input: ModelToolInput::Json(json!({"path": "README.md"})),
        }),
        ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId("call-1".to_string()),
            content: vec![
                ModelToolResultContent::Text("file contents".to_string()),
                ModelToolResultContent::Json(json!({"lines": 12})),
                ModelToolResultContent::DiscoveredTools(vec![tool]),
                ModelToolResultContent::Image {
                    source: ModelMediaSource::Bytes {
                        media_type: "image/png".to_string(),
                        data: Arc::from(vec![0x89, b'P', b'N', b'G', 0, 0xff]),
                    },
                    detail: Some(ModelImageDetail::Original),
                },
                ModelToolResultContent::Audio {
                    source: ModelMediaSource::Uri("data:audio/wav;base64,AAE=".to_string()),
                },
            ],
            is_error: None,
        }),
    ];

    for expected in items {
        let encoded = serde_json::to_string(&expected).unwrap();
        let actual: ModelInputItem = serde_json::from_str(&encoded).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn tool_input_and_result_keep_json_and_text_distinct() {
    let json_literal =
        r#"{"fraction":1.2300000000000000001,"large":18446744073709551616000000000000000000}"#;
    let large_json = serde_json::from_str::<Value>(json_literal).unwrap();
    let items = [
        ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId("json-call".to_string()),
            namespace: None,
            name: "json_tool".to_string(),
            input: ModelToolInput::Json(large_json.clone()),
        }),
        ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId("json-call".to_string()),
            namespace: None,
            name: "json_tool".to_string(),
            input: ModelToolInput::Text(json_literal.to_string()),
        }),
        ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId("json-result".to_string()),
            content: vec![ModelToolResultContent::Json(large_json.clone())],
            is_error: Some(false),
        }),
        ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId("json-result".to_string()),
            content: vec![ModelToolResultContent::Text(json_literal.to_string())],
            is_error: Some(false),
        }),
    ];

    let encoded: Vec<_> = items
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_ne!(encoded[0], encoded[1]);
    assert_ne!(encoded[2], encoded[3]);
    assert_eq!(
        encoded[0].pointer("/tool_call/input/json"),
        Some(&large_json)
    );
    assert_eq!(
        encoded[1].pointer("/tool_call/input/text"),
        Some(&json!(json_literal))
    );
    assert_eq!(
        encoded[2].pointer("/tool_result/content/0/json"),
        Some(&large_json)
    );
    assert_eq!(
        encoded[3].pointer("/tool_result/content/0/text"),
        Some(&json!(json_literal))
    );
    for expected in items {
        let serialized = serde_json::to_string(&expected).unwrap();
        let actual: ModelInputItem = serde_json::from_str(&serialized).unwrap();
        assert_eq!(actual, expected);
    }

    assert_eq!(serde_json::to_string(&large_json).unwrap(), json_literal);
}

#[test]
fn media_sources_keep_bytes_distinct_from_data_url_uris() {
    let binary_data: Arc<[u8]> = Arc::from(vec![0, 0xff, 0x80, 1]);
    let bytes_source = ModelMediaSource::Bytes {
        media_type: "image/png".to_string(),
        data: binary_data,
    };
    let uri_source = ModelMediaSource::Uri("data:image/png;base64,AP+AAQ==".to_string());
    let empty_bytes_source = ModelMediaSource::Bytes {
        media_type: "image/png".to_string(),
        data: Arc::from(Vec::<u8>::new()),
    };
    let items = [
        ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![
                ModelContent::Image {
                    source: bytes_source.clone(),
                    detail: Some(ModelImageDetail::Auto),
                },
                ModelContent::Image {
                    source: uri_source.clone(),
                    detail: Some(ModelImageDetail::Auto),
                },
                ModelContent::Image {
                    source: empty_bytes_source,
                    detail: Some(ModelImageDetail::Low),
                },
            ],
        }),
        ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId("media-result".to_string()),
            content: vec![
                ModelToolResultContent::Image {
                    source: bytes_source.clone(),
                    detail: None,
                },
                ModelToolResultContent::Image {
                    source: uri_source.clone(),
                    detail: None,
                },
            ],
            is_error: None,
        }),
    ];

    assert_ne!(
        serde_json::to_value(&bytes_source).unwrap(),
        serde_json::to_value(&uri_source).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&bytes_source).unwrap(),
        json!({"bytes": {"media_type": "image/png", "data": [0, 255, 128, 1]}})
    );
    for expected in items {
        let encoded = serde_json::to_value(&expected).unwrap();
        let actual: ModelInputItem = serde_json::from_value(encoded).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn tool_error_status_preserves_all_three_states() {
    let results = [None, Some(false), Some(true)].map(|is_error| {
        ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId("status-call".to_string()),
            content: vec![ModelToolResultContent::Text("result".to_string())],
            is_error,
        })
    });

    let encoded: Vec<_> = results
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .unwrap();
    for (expected, encoded) in results.into_iter().zip(encoded) {
        let actual: ModelInputItem = serde_json::from_value(encoded).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn function_output_schema_preserves_absent_null_and_value() {
    let declarations =
        [None, Some(Value::Null), Some(json!({"type": "string"}))].map(|output_schema| {
            ModelToolSpec::Function {
                namespace: None,
                name: "discover".to_string(),
                description: "Discover tools".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema,
                strict: false,
                availability: ModelToolAvailability::Deferred,
                purpose: ModelToolPurpose::Discovery,
            }
        });
    let serialized: Vec<_> = declarations
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<_, _>>()
        .unwrap();
    let encoded: Vec<Value> = serialized
        .iter()
        .map(String::as_str)
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .unwrap();

    assert!(encoded[0].pointer("/function/output_schema").is_none());
    assert_eq!(
        encoded[1].pointer("/function/output_schema"),
        Some(&Value::Null)
    );
    assert_eq!(
        encoded[2].pointer("/function/output_schema"),
        Some(&json!({"type": "string"}))
    );
    assert_ne!(encoded[0], encoded[1]);
    assert_ne!(encoded[1], encoded[2]);

    for (expected, serialized) in declarations.into_iter().zip(serialized) {
        let actual: ModelToolSpec = serde_json::from_str(&serialized).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn model_input_item_schema_contains_distinct_snake_case_variants() {
    let schema = serde_json::to_value(schema_for!(ModelInputItem)).unwrap();
    let alternatives = schema["oneOf"].as_array().unwrap();
    let variant_tags: BTreeSet<_> = alternatives
        .iter()
        .map(|variant| {
            let properties = variant["properties"].as_object().unwrap();
            assert_eq!(properties.len(), 1);
            properties.keys().next().unwrap().as_str()
        })
        .collect();

    assert_eq!(
        variant_tags,
        BTreeSet::from(["message", "reasoning", "tool_call", "tool_result"])
    );
}

#[test]
fn input_item_serialization_uses_external_snake_case_tags() {
    let item = ModelInputItem::ToolCall(ModelToolCall {
        call_id: ModelToolCallId("call-1".to_string()),
        namespace: None,
        name: "shell".to_string(),
        input: ModelToolInput::Text("pwd".to_string()),
    });

    assert_eq!(
        serde_json::to_value(item).unwrap(),
        json!({
            "tool_call": {
                "call_id": "call-1",
                "namespace": null,
                "name": "shell",
                "input": {"text": "pwd"}
            }
        })
    );
}
