use crate::model_runtime::ir::ModelImageDetail;
use crate::model_runtime::ir::ModelMediaSource;
use crate::model_runtime::ir::ModelToolResult;
use crate::model_runtime::ir::ModelToolResultContent;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseInputItem;
use codex_tools::DiscoveredFreeformInputFormat;
use codex_tools::DiscoveredToolAvailability;
use codex_tools::DiscoveredToolSpec;
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
        ToolResultContent::DiscoveredTools(tools) => Some(ModelToolResultContent::DiscoveredTools(
            tools
                .into_iter()
                .map(model_discovered_tool_spec)
                .collect::<Option<Vec<_>>>()?,
        )),
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
        ToolPayload::ToolSearch { .. } => {
            if result.is_error != Some(false) {
                return None;
            }
            let [ModelToolResultContent::DiscoveredTools(tools)] = result.content.as_slice() else {
                return None;
            };
            let tools = super::codex_request::tool_search_output_values_from_model(tools)?;
            Some(ResponseInputItem::ToolSearchOutput {
                call_id: result.call_id.0.clone(),
                status: "completed".to_string(),
                execution: "client".to_string(),
                tools,
            })
        }
    }
}

fn model_discovered_tool_spec(
    tool: DiscoveredToolSpec,
) -> Option<crate::model_runtime::ir::ModelToolSpec> {
    match tool {
        DiscoveredToolSpec::Function {
            namespace,
            name,
            description,
            input_schema,
            strict,
            availability,
        } => Some(crate::model_runtime::ir::ModelToolSpec::Function {
            namespace,
            name,
            description,
            input_schema,
            strict,
            availability: model_tool_availability(availability),
            purpose: crate::model_runtime::ir::ModelToolPurpose::Invocation,
        }),
        DiscoveredToolSpec::Freeform {
            namespace,
            name,
            description,
            input_format,
            availability,
        } => {
            // Codex client discovery only carries free-form tools inside a loadable namespace.
            // Keep broader neutral results on the legacy path instead of inventing a top-level
            // Responses ToolSearchOutput shape that the current producer cannot emit.
            let namespace = namespace?;
            Some(crate::model_runtime::ir::ModelToolSpec::Freeform {
                namespace: Some(namespace),
                name,
                description,
                input_format: model_freeform_input_format(input_format),
                availability: model_tool_availability(availability),
                purpose: crate::model_runtime::ir::ModelToolPurpose::Invocation,
            })
        }
    }
}

fn model_tool_availability(
    availability: DiscoveredToolAvailability,
) -> crate::model_runtime::ir::ModelToolAvailability {
    match availability {
        DiscoveredToolAvailability::Immediate => {
            crate::model_runtime::ir::ModelToolAvailability::Immediate
        }
        DiscoveredToolAvailability::Deferred => {
            crate::model_runtime::ir::ModelToolAvailability::Deferred
        }
    }
}

fn model_freeform_input_format(
    input_format: DiscoveredFreeformInputFormat,
) -> crate::model_runtime::ir::ModelFreeformInputFormat {
    match input_format {
        DiscoveredFreeformInputFormat::Grammar { syntax, definition } => {
            crate::model_runtime::ir::ModelFreeformInputFormat::Grammar { syntax, definition }
        }
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
        success: result.is_error.map(|is_error| !is_error),
    })
}

fn response_content_item(
    content: &ModelToolResultContent,
) -> Option<FunctionCallOutputContentItem> {
    match content {
        ModelToolResultContent::Text(text) => {
            Some(FunctionCallOutputContentItem::InputText { text: text.clone() })
        }
        // JSON is stringified only at this Codex adapter boundary.
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
