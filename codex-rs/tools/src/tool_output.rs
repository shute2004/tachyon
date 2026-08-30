use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use serde_json::Value as JsonValue;

use crate::ToolPayload;

/// Provider-neutral semantic result produced by a tool runtime.
/// Unrepresentable or provider-private output remains on the legacy response-item path.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<ToolResultContent>,
    /// Whether the result is known to be an error. `None` preserves a producer's unknown status.
    pub is_error: Option<bool>,
}

impl ToolResult {
    pub fn success_text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent::Text(text.into())],
            is_error: Some(false),
        }
    }

    pub fn error_text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent::Text(text.into())],
            is_error: Some(true),
        }
    }

    pub fn unknown_text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent::Text(text.into())],
            is_error: None,
        }
    }

    pub fn success_json(value: JsonValue) -> Self {
        Self {
            content: vec![ToolResultContent::Json(value)],
            is_error: Some(false),
        }
    }

    pub fn error_json(value: JsonValue) -> Self {
        Self {
            content: vec![ToolResultContent::Json(value)],
            is_error: Some(true),
        }
    }

    pub fn unknown_json(value: JsonValue) -> Self {
        Self {
            content: vec![ToolResultContent::Json(value)],
            is_error: None,
        }
    }

    pub fn success_discovered_tools(tools: Vec<DiscoveredToolSpec>) -> Self {
        Self {
            content: vec![ToolResultContent::DiscoveredTools(tools)],
            is_error: Some(false),
        }
    }

    pub fn from_function_call_output(payload: &FunctionCallOutputPayload) -> Option<Self> {
        let content = match &payload.body {
            FunctionCallOutputBody::Text(text) => vec![ToolResultContent::Text(text.clone())],
            FunctionCallOutputBody::ContentItems(items)
                if matches!(
                    items.as_slice(),
                    [FunctionCallOutputContentItem::InputText { .. }]
                ) =>
            {
                return None;
            }
            FunctionCallOutputBody::ContentItems(items) => items
                .iter()
                .map(tool_result_content)
                .collect::<Option<Vec<_>>>()?,
        };

        Some(Self {
            content,
            is_error: payload.success.map(|success| !success),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolResultContent {
    Text(String),
    Json(JsonValue),
    /// Semantic tool declarations returned by a client-side discovery operation.
    ///
    /// This is deliberately separate from `LoadableToolSpec` and does not carry a provider wire
    /// representation. The Codex adapter projects it into its Responses-shaped output only at the
    /// model-runtime boundary.
    DiscoveredTools(Vec<DiscoveredToolSpec>),
    Image {
        uri: String,
        detail: Option<ToolResultImageDetail>,
    },
    Audio {
        uri: String,
    },
}

/// A tool declaration exposed by a client-side discovery result.
#[derive(Clone, Debug, PartialEq)]
pub enum DiscoveredToolSpec {
    Function {
        namespace: Option<String>,
        name: String,
        description: String,
        input_schema: JsonValue,
        strict: bool,
        availability: DiscoveredToolAvailability,
    },
    Freeform {
        namespace: Option<String>,
        name: String,
        description: String,
        input_format: DiscoveredFreeformInputFormat,
        availability: DiscoveredToolAvailability,
    },
}

/// Whether a discovered tool is immediately visible or deferred until discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveredToolAvailability {
    Immediate,
    Deferred,
}

/// Input constraint for a discovered free-form tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveredFreeformInputFormat {
    Grammar { syntax: String, definition: String },
}

/// Image fidelity hint that does not assume a provider wire representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolResultImageDetail {
    Auto,
    Low,
    High,
    Original,
}

/// Model-facing output contract returned by executable tool runtimes.
pub trait ToolOutput: Send {
    /// Returns a deliberately lossy diagnostic representation, before telemetry size limits.
    /// Implementations may summarize results or omit media and encrypted content. This is not
    /// the authoritative tool result; an untruncated log does not imply a complete result.
    /// The logger owns the additional configurable byte limit.
    fn log_output(&self) -> String;

    fn success_for_logging(&self) -> bool;

    /// Whether this output contains external context that should disable memory generation when
    /// `memories.disable_on_external_context` is enabled.
    fn contains_external_context(&self) -> bool {
        false
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem;

    /// Returns `None` when canonical projection cannot preserve exact semantics.
    fn to_tool_result(&self) -> Option<ToolResult> {
        None
    }

    /// Returns the tool call id exposed to `PostToolUse` hooks for this output.
    fn post_tool_use_id(&self, call_id: &str) -> String {
        call_id.to_string()
    }

    /// Returns the tool input exposed to `PostToolUse` hooks for this output.
    fn post_tool_use_input(&self, _payload: &ToolPayload) -> Option<JsonValue> {
        None
    }

    /// Returns the stable value exposed to `PostToolUse` hooks for this tool output.
    ///
    /// Tool handlers decide whether a tool participates in `PostToolUse`, but
    /// this method lets the output type own any conversion from model-facing
    /// response content to hook-facing data. Returning `None` means the output
    /// should not produce a post-use hook payload, not merely that the tool had
    /// empty output.
    fn post_tool_use_response(&self, _call_id: &str, _payload: &ToolPayload) -> Option<JsonValue> {
        None
    }

    fn code_mode_result(&self, payload: &ToolPayload) -> JsonValue {
        response_input_to_code_mode_result(self.to_response_item("", payload))
    }
}

impl<T> ToolOutput for Box<T>
where
    T: ToolOutput + ?Sized,
{
    fn log_output(&self) -> String {
        (**self).log_output()
    }

    fn success_for_logging(&self) -> bool {
        (**self).success_for_logging()
    }

    fn contains_external_context(&self) -> bool {
        (**self).contains_external_context()
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        (**self).to_response_item(call_id, payload)
    }

    fn to_tool_result(&self) -> Option<ToolResult> {
        (**self).to_tool_result()
    }

    fn post_tool_use_id(&self, call_id: &str) -> String {
        (**self).post_tool_use_id(call_id)
    }

    fn post_tool_use_input(&self, payload: &ToolPayload) -> Option<JsonValue> {
        (**self).post_tool_use_input(payload)
    }

    fn post_tool_use_response(&self, call_id: &str, payload: &ToolPayload) -> Option<JsonValue> {
        (**self).post_tool_use_response(call_id, payload)
    }

    fn code_mode_result(&self, payload: &ToolPayload) -> JsonValue {
        (**self).code_mode_result(payload)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsonToolOutput {
    value: JsonValue,
    success: Option<bool>,
    contains_external_context: bool,
}

impl JsonToolOutput {
    pub fn new(value: JsonValue) -> Self {
        Self {
            value,
            success: Some(true),
            contains_external_context: false,
        }
    }

    pub fn with_success(value: JsonValue, success: Option<bool>) -> Self {
        Self {
            value,
            success,
            contains_external_context: false,
        }
    }

    pub fn with_external_context(mut self) -> Self {
        self.contains_external_context = true;
        self
    }
}

impl ToolOutput for JsonToolOutput {
    fn log_output(&self) -> String {
        self.value.to_string()
    }

    fn success_for_logging(&self) -> bool {
        self.success.unwrap_or(true)
    }

    fn contains_external_context(&self) -> bool {
        self.contains_external_context
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        let output = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(self.value.to_string()),
            success: self.success,
        };

        if matches!(payload, ToolPayload::Custom { .. }) {
            return ResponseInputItem::CustomToolCallOutput {
                call_id: call_id.to_string(),
                name: None,
                output,
            };
        }

        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output,
        }
    }

    fn to_tool_result(&self) -> Option<ToolResult> {
        Some(match self.success {
            Some(true) => ToolResult::success_json(self.value.clone()),
            Some(false) => ToolResult::error_json(self.value.clone()),
            None => ToolResult::unknown_json(self.value.clone()),
        })
    }

    fn post_tool_use_response(&self, _call_id: &str, _payload: &ToolPayload) -> Option<JsonValue> {
        Some(self.value.clone())
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        self.value.clone()
    }
}

impl ToolOutput for codex_protocol::mcp::CallToolResult {
    fn log_output(&self) -> String {
        let output = self.as_function_call_output_payload();
        // Do not fall back to serializing media or encrypted content into logs.
        output.body.to_text().unwrap_or_default()
    }

    fn success_for_logging(&self) -> bool {
        self.success()
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::McpToolCallOutput {
            call_id: call_id.to_string(),
            output: self.clone(),
        }
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        let mut result = serde_json::to_value(self).unwrap_or_else(|err| {
            JsonValue::String(format!("failed to serialize mcp result: {err}"))
        });
        // MCP result metadata is private to clients and must not reach Code Mode.
        if let JsonValue::Object(fields) = &mut result {
            fields.remove("_meta");
        }
        result
    }
}

fn tool_result_content(content: &FunctionCallOutputContentItem) -> Option<ToolResultContent> {
    match content {
        FunctionCallOutputContentItem::InputText { text } => {
            Some(ToolResultContent::Text(text.clone()))
        }
        FunctionCallOutputContentItem::InputImage { image_url, detail } => {
            Some(ToolResultContent::Image {
                uri: image_url.clone(),
                detail: detail.map(tool_result_image_detail),
            })
        }
        FunctionCallOutputContentItem::InputAudio { audio_url } => Some(ToolResultContent::Audio {
            uri: audio_url.clone(),
        }),
        FunctionCallOutputContentItem::EncryptedContent { .. } => None,
    }
}

fn tool_result_image_detail(detail: codex_protocol::models::ImageDetail) -> ToolResultImageDetail {
    match detail {
        codex_protocol::models::ImageDetail::Auto => ToolResultImageDetail::Auto,
        codex_protocol::models::ImageDetail::Low => ToolResultImageDetail::Low,
        codex_protocol::models::ImageDetail::High => ToolResultImageDetail::High,
        codex_protocol::models::ImageDetail::Original => ToolResultImageDetail::Original,
    }
}

fn response_input_to_code_mode_result(response: ResponseInputItem) -> JsonValue {
    match response {
        ResponseInputItem::Message { content, .. } => content_items_to_code_mode_result(
            &content
                .into_iter()
                .map(|item| match item {
                    codex_protocol::models::ContentItem::InputText { text }
                    | codex_protocol::models::ContentItem::OutputText { text } => {
                        FunctionCallOutputContentItem::InputText { text }
                    }
                    codex_protocol::models::ContentItem::InputImage { image_url, detail } => {
                        FunctionCallOutputContentItem::InputImage {
                            image_url,
                            detail: detail.or(Some(DEFAULT_IMAGE_DETAIL)),
                        }
                    }
                    codex_protocol::models::ContentItem::InputAudio { audio_url } => {
                        FunctionCallOutputContentItem::InputAudio { audio_url }
                    }
                })
                .collect::<Vec<_>>(),
        ),
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => match output.body {
            FunctionCallOutputBody::Text(text) => JsonValue::String(text),
            FunctionCallOutputBody::ContentItems(items) => {
                content_items_to_code_mode_result(&items)
            }
        },
        ResponseInputItem::ToolSearchOutput { tools, .. } => JsonValue::Array(tools),
        ResponseInputItem::McpToolCallOutput { output, .. } => serde_json::to_value(output)
            .unwrap_or_else(|err| {
                JsonValue::String(format!("failed to serialize mcp result: {err}"))
            }),
    }
}

fn content_items_to_code_mode_result(items: &[FunctionCallOutputContentItem]) -> JsonValue {
    JsonValue::String(
        items
            .iter()
            .filter_map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } if !text.trim().is_empty() => {
                    Some(text.clone())
                }
                FunctionCallOutputContentItem::InputImage { image_url, .. }
                    if !image_url.trim().is_empty() =>
                {
                    Some(image_url.clone())
                }
                FunctionCallOutputContentItem::InputAudio { audio_url }
                    if !audio_url.trim().is_empty() =>
                {
                    Some(audio_url.clone())
                }
                FunctionCallOutputContentItem::InputText { .. }
                | FunctionCallOutputContentItem::InputImage { .. }
                | FunctionCallOutputContentItem::InputAudio { .. }
                | FunctionCallOutputContentItem::EncryptedContent { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
#[path = "tool_output_tests.rs"]
mod tests;
