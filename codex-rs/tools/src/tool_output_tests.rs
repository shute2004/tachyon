use super::JsonToolOutput;
use super::ToolOutput;
use super::ToolResult;
use super::ToolResultContent;
use super::ToolResultImageDetail;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use serde_json::json;

const IMAGE: &str = "https://example.test/image.png";
const AUDIO: &str = "https://example.test/audio.wav";

fn result(content: Vec<ToolResultContent>, is_error: bool) -> ToolResult {
    ToolResult { content, is_error }
}

fn text_result(text: &str, is_error: bool) -> ToolResult {
    result(vec![ToolResultContent::Text(text.into())], is_error)
}

fn json_result(value: serde_json::Value, is_error: bool) -> ToolResult {
    result(vec![ToolResultContent::Json(value)], is_error)
}

fn payload(body: FunctionCallOutputBody, success: Option<bool>) -> FunctionCallOutputPayload {
    FunctionCallOutputPayload { body, success }
}

fn item(value: serde_json::Value) -> FunctionCallOutputContentItem {
    serde_json::from_value(value).unwrap()
}

#[test]
fn constructors_and_json_outputs() {
    assert_eq!(ToolResult::success_text("ok"), text_result("ok", false));
    assert_eq!(
        ToolResult::error_text("failed"),
        text_result("failed", true)
    );
    let ok = json!({"answer": 42});
    let error = json!({"error": "failed"});
    assert_eq!(
        ToolResult::success_json(ok.clone()),
        json_result(ok.clone(), false)
    );
    assert_eq!(
        ToolResult::error_json(error.clone()),
        json_result(error.clone(), true)
    );
    assert_eq!(
        JsonToolOutput::with_success(ok.clone(), Some(true)).to_tool_result(),
        Some(json_result(ok, false))
    );
    assert_eq!(
        JsonToolOutput::with_success(error.clone(), Some(false)).to_tool_result(),
        Some(json_result(error, true))
    );
    assert_eq!(
        JsonToolOutput::with_success(json!("unknown"), None).to_tool_result(),
        None
    );
}

#[test]
fn function_call_output_projects_content_or_falls_back() {
    for success in [true, false] {
        assert_eq!(
            ToolResult::from_function_call_output(&payload(
                FunctionCallOutputBody::Text("output".into()),
                Some(success),
            )),
            Some(text_result("output", !success))
        );
    }

    let media_payload = payload(
        FunctionCallOutputBody::ContentItems(vec![
            item(json!({"type":"input_text","text":"before"})),
            item(json!({"type":"input_image","image_url":IMAGE,"detail":"original"})),
            item(json!({"type":"input_audio","audio_url":AUDIO})),
            item(json!({"type":"input_text","text":"after"})),
        ]),
        Some(false),
    );
    let actual = ToolResult::from_function_call_output(&media_payload).unwrap();
    assert!(actual.is_error);
    assert!(matches!(
        actual.content.as_slice(),
        [
            ToolResultContent::Text(before),
            ToolResultContent::Image {
                uri: image,
                detail: Some(ToolResultImageDetail::Original)
            },
            ToolResultContent::Audio { uri: audio },
            ToolResultContent::Text(after),
        ] if before == "before" && image == IMAGE && audio == AUDIO && after == "after"
    ));

    let encrypted = payload(
        FunctionCallOutputBody::ContentItems(vec![
            item(json!({"type":"input_text","text":"visible"})),
            item(json!({"type":"encrypted_content","encrypted_content":"opaque"})),
        ]),
        Some(true),
    );
    let unknown = payload(FunctionCallOutputBody::Text("unknown".into()), None);
    for payload in [encrypted, unknown] {
        assert_eq!(ToolResult::from_function_call_output(&payload), None);
    }
}
