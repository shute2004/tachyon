use super::model_tool_result;
use super::to_response_item;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolResult;
use crate::model_runtime::ir::ModelToolResultContent;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolPayload;
use codex_tools::ToolResult;
use codex_tools::ToolResultContent;
use serde_json::json;

#[test]
fn neutral_json_result_stays_structured_until_the_adapter_boundary() {
    let value = json!({"answer": 42});
    let result = ToolResult {
        content: vec![ToolResultContent::Json(value.clone())],
        is_error: true,
    };

    assert_eq!(
        model_tool_result(result.clone(), "json-call"),
        Some(ModelToolResult {
            call_id: ModelToolCallId("json-call".to_string()),
            content: vec![ModelToolResultContent::Json(value)],
            is_error: true,
        })
    );

    assert_eq!(
        to_response_item(
            result,
            "json-call",
            &ToolPayload::Function {
                arguments: "{}".to_string(),
            },
        ),
        Some(ResponseInputItem::FunctionCallOutput {
            call_id: "json-call".to_string(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(r#"{"answer":42}"#.to_string()),
                success: Some(false),
            },
        })
    );
}

#[test]
fn function_and_custom_results_encode_exact_body_and_success() {
    for (payload, success, text) in [
        (
            ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            true,
            "function success",
        ),
        (
            ToolPayload::Function {
                arguments: "{}".to_string(),
            },
            false,
            "function error",
        ),
        (
            ToolPayload::Custom {
                input: "custom input".to_string(),
            },
            true,
            "custom success",
        ),
        (
            ToolPayload::Custom {
                input: "custom input".to_string(),
            },
            false,
            "custom error",
        ),
    ] {
        let result = FunctionToolOutput::from_text(text.to_string(), Some(success))
            .to_tool_result()
            .expect("known function output success should project");
        let output = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(text.to_string()),
            success: Some(success),
        };
        let expected = match &payload {
            ToolPayload::Function { .. } => ResponseInputItem::FunctionCallOutput {
                call_id: "call-1".to_string(),
                output,
            },
            ToolPayload::Custom { .. } => ResponseInputItem::CustomToolCallOutput {
                call_id: "call-1".to_string(),
                name: None,
                output,
            },
            ToolPayload::ToolSearch { .. } => {
                panic!("function output test must use function or custom payload")
            }
        };

        assert_eq!(to_response_item(result, "call-1", &payload), Some(expected));
    }
}

#[test]
fn mixed_text_image_audio_output_preserves_order_and_image_detail() {
    for detail in [
        ImageDetail::Auto,
        ImageDetail::Low,
        ImageDetail::High,
        ImageDetail::Original,
    ] {
        let output = FunctionToolOutput::from_content(
            vec![
                FunctionCallOutputContentItem::InputText {
                    text: "before".to_string(),
                },
                FunctionCallOutputContentItem::InputImage {
                    image_url: "https://example.test/image.png".to_string(),
                    detail: Some(detail),
                },
                FunctionCallOutputContentItem::InputAudio {
                    audio_url: "https://example.test/audio.wav".to_string(),
                },
                FunctionCallOutputContentItem::InputText {
                    text: "after".to_string(),
                },
            ],
            Some(false),
        );
        let result = output
            .to_tool_result()
            .expect("multimodal function output should project");

        assert_eq!(
            to_response_item(
                result,
                "media-call",
                &ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
            ),
            Some(ResponseInputItem::FunctionCallOutput {
                call_id: "media-call".to_string(),
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::ContentItems(vec![
                        FunctionCallOutputContentItem::InputText {
                            text: "before".to_string(),
                        },
                        FunctionCallOutputContentItem::InputImage {
                            image_url: "https://example.test/image.png".to_string(),
                            detail: Some(detail),
                        },
                        FunctionCallOutputContentItem::InputAudio {
                            audio_url: "https://example.test/audio.wav".to_string(),
                        },
                        FunctionCallOutputContentItem::InputText {
                            text: "after".to_string(),
                        },
                    ]),
                    success: Some(false),
                },
            })
        );
    }
}

#[test]
fn context_projection_uses_strict_fallback_for_unknown_function_outputs() {
    assert_eq!(
        FunctionToolOutput::from_text("unknown".to_string(), None).to_tool_result(),
        None
    );
    assert_eq!(
        FunctionToolOutput::from_content(
            vec![FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "opaque".to_string(),
            }],
            Some(true),
        )
        .to_tool_result(),
        None
    );
}
