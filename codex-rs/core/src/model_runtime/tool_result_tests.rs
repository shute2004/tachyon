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
use codex_tools::JsonToolOutput;
use codex_tools::ToolPayload;
use codex_tools::ToolResult;
use codex_tools::ToolResultContent;
use serde_json::json;

fn media_items(detail: ImageDetail) -> Vec<FunctionCallOutputContentItem> {
    serde_json::from_value(json!([
        {"type": "input_text", "text": "before"},
        {"type": "input_image", "image_url": "image", "detail": detail},
        {"type": "input_audio", "audio_url": "audio"},
        {"type": "input_text", "text": "after"}
    ]))
    .unwrap()
}

fn output(actual: ResponseInputItem, payload: &ToolPayload) -> (String, FunctionCallOutputPayload) {
    match (payload, actual) {
        (
            ToolPayload::Function { .. },
            ResponseInputItem::FunctionCallOutput { call_id, output },
        )
        | (
            ToolPayload::Custom { .. },
            ResponseInputItem::CustomToolCallOutput {
                call_id, output, ..
            },
        ) => (call_id, output),
        (_, actual) => panic!("unexpected tool output: {actual:?}"),
    }
}

#[test]
fn neutral_json_result_stays_structured_until_the_adapter_boundary() {
    let value = json!({"answer": 42});
    let result = ToolResult {
        content: vec![ToolResultContent::Json(value.clone())],
        is_error: Some(true),
    };
    let expected = ModelToolResult {
        call_id: ModelToolCallId("json-call".to_string()),
        content: vec![ModelToolResultContent::Json(value)],
        is_error: Some(true),
    };
    assert_eq!(
        model_tool_result(result.clone(), "json-call"),
        Some(expected)
    );
    let payload = ToolPayload::Function {
        arguments: "{}".into(),
    };
    let (call_id, output) = output(
        to_response_item(result, "json-call", &payload).unwrap(),
        &payload,
    );
    assert_eq!(call_id, "json-call");
    assert_eq!(
        output.body,
        FunctionCallOutputBody::Text(r#"{"answer":42}"#.into())
    );
    assert_eq!(output.success, Some(false));
}

#[test]
fn text_result_adapter_preserves_all_error_status_values() {
    let payload = ToolPayload::Function {
        arguments: "{}".into(),
    };

    for success in [Some(true), Some(false), None] {
        let result = ToolResult::from_function_call_output(&FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text("result".into()),
            success,
        })
        .expect("text output should be canonicalizable");
        assert_eq!(result.is_error, success.map(|success| !success));

        let (_, output) = output(
            to_response_item(result, "status-call", &payload).unwrap(),
            &payload,
        );
        assert_eq!(output.body, FunctionCallOutputBody::Text("result".into()));
        assert_eq!(output.success, success);
    }
}

#[test]
fn unknown_json_and_function_outputs_use_canonical_egress() {
    let payload = ToolPayload::Function {
        arguments: "{}".into(),
    };

    let json_output = JsonToolOutput::with_success(json!({"pending": true}), None);
    let expected = json_output.to_response_item("json-call", &payload);
    let result = json_output
        .to_tool_result()
        .expect("unknown JSON output should project");
    assert_eq!(result.is_error, None);
    assert_eq!(
        to_response_item(result, "json-call", &payload),
        Some(expected)
    );

    let function_output = FunctionToolOutput::from_text("pending".into(), None);
    let expected = function_output.to_response_item("function-call", &payload);
    let result = function_output
        .to_tool_result()
        .expect("unknown function output should project");
    assert_eq!(result.is_error, None);
    assert_eq!(
        to_response_item(result, "function-call", &payload),
        Some(expected)
    );
}

#[test]
fn function_and_custom_results_encode_exact_body_and_success() {
    for payload in [
        ToolPayload::Function {
            arguments: "{}".into(),
        },
        ToolPayload::Custom {
            input: "custom".into(),
        },
    ] {
        for success in [true, false] {
            let text = if success { "success" } else { "error" };
            let result = FunctionToolOutput::from_text(text.into(), Some(success))
                .to_tool_result()
                .expect("known function output success should project");
            let (call_id, output) = output(
                to_response_item(result, "call-1", &payload).unwrap(),
                &payload,
            );
            assert_eq!(call_id, "call-1");
            assert_eq!(output.body, FunctionCallOutputBody::Text(text.into()));
            assert_eq!(output.success, Some(success));
        }
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
        let items = media_items(detail);
        let result = FunctionToolOutput::from_content(items.clone(), Some(false))
            .to_tool_result()
            .expect("multimodal output should project");
        let payload = ToolPayload::Function {
            arguments: "{}".into(),
        };
        let (call_id, output) = output(
            to_response_item(result, "media-call", &payload).unwrap(),
            &payload,
        );
        assert_eq!(call_id, "media-call");
        assert_eq!(output.body, FunctionCallOutputBody::ContentItems(items));
        assert_eq!(output.success, Some(false));
    }
}
