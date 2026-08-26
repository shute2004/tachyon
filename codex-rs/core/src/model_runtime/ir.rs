//! Canonical model request/event vocabulary owned by the Tachyon kernel.
//!
//! These types describe model-execution semantics that are useful across providers. They are
//! intentionally narrower than Codex's existing Responses-shaped `Prompt`, `ResponseItem`, and
//! `ResponseEvent` types. Provider/product metadata and provider-private continuation state stay
//! below the model-runtime boundary and must not be added here merely to preserve one wire format.
//!
//! The first IR slice is definition-only: existing Codex call sites continue to use the migration
//! compatibility types until the adapter conversions are wired in focused follow-up changes.

use std::sync::Arc;

use serde_json::Value;

/// One provider-neutral model request issued by the harness.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    /// Harness/developer instructions that apply to this request.
    pub instructions: String,
    /// Ordered conversation and tool-continuation input visible to the model.
    pub input: Vec<ModelInputItem>,
    /// Tools the model may call for this request.
    pub tools: Vec<ModelToolSpec>,
    /// Whether the model may request multiple tool calls without waiting for an intermediate turn.
    pub parallel_tool_calls: bool,
    /// Desired model output shape.
    pub output: ModelOutputConfig,
}

impl Default for ModelRequest {
    fn default() -> Self {
        Self {
            instructions: String::new(),
            input: Vec::new(),
            tools: Vec::new(),
            parallel_tool_calls: false,
            output: ModelOutputConfig::default(),
        }
    }
}

/// One durable request-side item in the provider-neutral model conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelInputItem {
    Message(ModelMessage),
    ToolCall(ModelToolCall),
    ToolResult(ModelToolResult),
}

/// A role-bearing model message.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelMessage {
    pub role: ModelMessageRole,
    pub content: Vec<ModelContent>,
}

/// Roles with stable meaning across the model-runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelMessageRole {
    System,
    Developer,
    User,
    Assistant,
}

/// Provider-neutral message content.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelContent {
    Text(String),
    Image {
        source: ModelMediaSource,
        detail: Option<ModelImageDetail>,
    },
    Audio {
        source: ModelMediaSource,
    },
}

/// Media input without assuming that a provider accepts data URLs, hosted URLs, or uploaded files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelMediaSource {
    Uri(String),
    Bytes {
        media_type: String,
        data: Arc<[u8]>,
    },
}

/// Optional image-fidelity hint. Adapters may reject or approximate unsupported hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelImageDetail {
    Auto,
    Low,
    High,
    Original,
}

/// Tool declaration exposed to a model.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelToolSpec {
    /// JSON-schema-described function call.
    Function {
        namespace: Option<String>,
        name: String,
        description: String,
        input_schema: Value,
        strict: bool,
    },
    /// Free-form textual tool input.
    Freeform {
        namespace: Option<String>,
        name: String,
        description: String,
    },
}

/// Correlation identifier for one logical tool call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelToolCallId(pub String);

/// Model-authored request to invoke a harness tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelToolCall {
    pub call_id: ModelToolCallId,
    pub namespace: Option<String>,
    pub name: String,
    pub input: ModelToolInput,
}

/// Tool-call input without assuming that every provider encodes function arguments as JSON text.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelToolInput {
    Json(Value),
    Text(String),
}

/// Harness-authored result for a previous model tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelToolResult {
    pub call_id: ModelToolCallId,
    pub content: Vec<ModelToolResultContent>,
    pub is_error: bool,
}

/// Tool-result content is distinct from ordinary message content because structured JSON output is
/// a first-class harness capability even when a provider's message content model is text/media only.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelToolResultContent {
    Text(String),
    Json(Value),
    Image {
        source: ModelMediaSource,
        detail: Option<ModelImageDetail>,
    },
    Audio {
        source: ModelMediaSource,
    },
}

/// Desired output contract for one model request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOutputConfig {
    pub format: ModelOutputFormat,
}

/// Output formats with provider-independent harness meaning.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ModelOutputFormat {
    #[default]
    Text,
    JsonSchema {
        name: Option<String>,
        schema: Value,
        strict: bool,
    },
}

/// Stable correlation identifier for one streamed model output item.
///
/// The identifier is a model-runtime correlation key, not a provider response/item identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelItemId(pub String);

/// Provider-neutral model output item.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelOutputItem {
    Message {
        id: ModelItemId,
        content: Vec<ModelContent>,
    },
    ToolCall {
        id: ModelItemId,
        call: ModelToolCall,
    },
    Reasoning {
        id: ModelItemId,
        summary: Vec<String>,
    },
}

/// One event emitted by a model execution stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    Started,
    OutputItemStarted(ModelOutputItem),
    OutputItemCompleted(ModelOutputItem),
    TextDelta {
        item_id: ModelItemId,
        delta: String,
    },
    ToolCallInputDelta {
        item_id: ModelItemId,
        call_id: ModelToolCallId,
        delta: String,
    },
    ReasoningDelta {
        item_id: ModelItemId,
        kind: ModelReasoningDeltaKind,
        delta: String,
        section_index: Option<u32>,
    },
    Completed(ModelCompletion),
}

/// Distinguishes model-visible reasoning summaries from optional richer reasoning content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelReasoningDeltaKind {
    Summary,
    Content,
}

/// Terminal execution metadata with provider-independent harness meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCompletion {
    pub usage: Option<ModelUsage>,
    /// Some providers can explicitly indicate that more model/tool continuation is expected.
    pub end_turn: Option<bool>,
}

/// Exact usage attributed to one upstream model request when the provider reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    /// Provider-reported total when available. It is not recomputed because providers may account
    /// for additional token classes that are not represented by the fields above.
    pub total_tokens: Option<u64>,
}

#[cfg(test)]
#[path = "ir_tests.rs"]
mod tests;
