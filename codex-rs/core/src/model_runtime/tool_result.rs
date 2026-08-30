//! Transitional egress from provider-neutral tool results to the Codex Responses adapter.
//! Provider-private or otherwise unrepresentable outputs return `None` for exact legacy fallback.

use crate::model_runtime::ir::ModelImageDetail;
use crate::model_runtime::ir::ModelMediaSource;
use crate::model_runtime::ir::ModelToolResult;
use crate::model_runtime::ir::ModelToolResultContent;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseInputItem;
use codex_tools::ToolPayload;
use codex_tools::ToolResult;
use codex_tools::ToolResultContent;
use codex_tools::ToolResultImageDetail;

/// Converts a neutral result when it has a lossless Responses representation.
pub(crate) fn to_response_item(
    result: ToolResult,
    call_id: &str,
    payload: &ToolPayload,
) -> Option<ResponseInputItem> {
    let result = model_tool_result(result, call_id)?;
    response_item_from_model_tool_result(&result, payload)
}

/// Projects a neutral result into canonical model IR before provider wire encoding.
fn model_tool_result(result: ToolResult, call_id: &str) -> Option<ModelToolResult> {
    let content = result
        .content
        .into_iter()
        .map(model_tool_result_content)
        .collect::<Option<Vec<_>>>()?;

    Some(ModelToolResult {
        call_id: crate::model_runtime::ir::ModelToolCallId(call_id.to_string()),
        content,
        is_error: result.is_error,
    })
}

fn model_tool_result_content(content: ToolResultContent) -> Option<ModelToolResultContent> {
    match content {
        ToolResultContent::Text(text) => Some(ModelToolResultContent::Text(text)),
        ToolResultContent::Json(value) => Some(ModelToolResultContent::Json(value)),
        ToolResultContent::Image { uri, detail } => Some(ModelToolResultContent::Image {
            source: ModelMediaSource::Uri(uri),
            detail: detail.map(model_image_detail),
        }),
        ToolResultContent::Audio { uri } => Some(ModelToolResultContent::Audio {
            source: ModelMediaSource::Uri(uri),
        }),
    }
}

fn model_image_detail(detail: ToolResultImageDetail) -> ModelImageDetail {
    match detail {
        ToolResultImageDetail::Auto => ModelImageDetail::Auto,
        ToolResultImageDetail::Low => ModelImageDetail::Low,
        ToolResultImageDetail::High => ModelImageDetail::High,
        ToolResultImageDetail::Original => ModelImageDetail::Original,
    }
}

fn response_item_from_model_tool_result(
    result: &ModelToolResult,
    payload: &ToolPayload,
) -> Option<ResponseInputItem> {
    match payload {
        ToolPayload::Function { .. } => Some(ResponseInputItem::FunctionCallOutput {
            call_id: result.call_id.0.clone(),
            output: function_call_output_payload(result)?,
        }),
        ToolPayload::Custom { .. } => Some(ResponseInputItem::CustomToolCallOutput {
            call_id: result.call_id.0.clone(),
            name: None,
            output: function_call_output_payload(result)?,
        }),
        ToolPayload::ToolSearch { .. } => None,
    }
}

fn function_call_output_payload(result: &ModelToolResult) -> Option<FunctionCallOutputPayload> {
    let content = result
        .content
        .iter()
        .map(response_content_item)
        .collect::<Option<Vec<_>>>()?;
    let body = match content.as_slice() {
        [FunctionCallOutputContentItem::InputText { text }] => {
            FunctionCallOutputBody::Text(text.clone())
        }
        _ => FunctionCallOutputBody::ContentItems(content),
    };

    Some(FunctionCallOutputPayload {
        body,
        success: Some(!result.is_error),
    })
}

fn response_content_item(
    content: &ModelToolResultContent,
) -> Option<FunctionCallOutputContentItem> {
    match content {
        ModelToolResultContent::Text(text) => {
            Some(FunctionCallOutputContentItem::InputText { text: text.clone() })
        }
        // JSON is intentionally stringified only at this Codex adapter boundary. It remains
        // `ModelToolResultContent::Json` while crossing the provider-neutral model runtime IR.
        ModelToolResultContent::Json(value) => Some(FunctionCallOutputContentItem::InputText {
            text: value.to_string(),
        }),
        ModelToolResultContent::Image { source, detail } => {
            Some(FunctionCallOutputContentItem::InputImage {
                image_url: response_media_uri(source)?,
                detail: detail.map(response_image_detail),
            })
        }
        ModelToolResultContent::Audio { source } => {
            Some(FunctionCallOutputContentItem::InputAudio {
                audio_url: response_media_uri(source)?,
            })
        }
        _ => None,
    }
}

fn response_media_uri(source: &ModelMediaSource) -> Option<String> {
    match source {
        ModelMediaSource::Uri(uri) => Some(uri.clone()),
        ModelMediaSource::Bytes { .. } => None,
    }
}

fn response_image_detail(detail: ModelImageDetail) -> ImageDetail {
    match detail {
        ModelImageDetail::Auto => ImageDetail::Auto,
        ModelImageDetail::Low => ImageDetail::Low,
        ModelImageDetail::High => ImageDetail::High,
        ModelImageDetail::Original => ImageDetail::Original,
    }
}

#[cfg(test)]
#[path = "tool_result_tests.rs"]
mod tests;
