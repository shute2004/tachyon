use super::{model_tool_result, to_response_item};
use crate::model_runtime::ir::{ModelToolCallId, ModelToolResult, ModelToolResultContent};
use crate::tools::context::{FunctionToolOutput, ToolOutput};
use codex_protocol::models::{
    FunctionCallOutputBody, FunctionCallOutputContentItem, FunctionCallOutputPayload, ImageDetail,
    ResponseInputItem,
};
use codex_tools::{ToolPayload, ToolResult, ToolResultContent};
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

fn response_output(actual: ResponseInputItem) -> (String, FunctionCallOutputPayload) {
    match actual {
        ResponseInputItem::FunctionCallOutput { call_id, output }
        | ResponseInputItem::CustomToolCallOutput {
            call_id, output, ..
        } => (call_id, output),
        actual => panic!("unexpected tool output: {actual:?}"),
    }
}

#[test]
fn neutral_json_result_stays_structured_until_the_adapter_boundary() {
    let value = json!({"answer": 42});
    let result = ToolResult {
        content: vec![ToolResultContent::Json(value.clone())],
        is_error: true,
    };
    let expected = ModelToolResult {
        call_id: ModelToolCallId("json-call".to_string()),
        content: vec![ModelToolResultContent::Json(value)],
        is_error: true,
    };
    assert_eq!(
        model_tool_result(result.clone(), "json-call"),
        Some(expected)
    );
    let payload = ToolPayload::Function {
        arguments: "{}".into(),
    };
    let (call_id, output) =
        response_output(to_response_item(result, "json-call", &payload).unwrap());
    assert_eq!(call_id, "json-call");
    assert_eq!(
        output.body,
        FunctionCallOutputBody::Text(r#"{"answer":42}"#.into())
    );
    assert_eq!(output.success, Some(false));
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
            let (call_id, output) =
                response_output(to_response_item(result, "call-1", &payload).unwrap());
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
        let (call_id, output) =
            response_output(to_response_item(result, "media-call", &payload).unwrap());
        assert_eq!(call_id, "media-call");
        assert_eq!(output.body, FunctionCallOutputBody::ContentItems(items));
        assert_eq!(output.success, Some(false));
    }
}
