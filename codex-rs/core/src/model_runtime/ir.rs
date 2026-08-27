//! Canonical model request/event vocabulary owned by the Tachyon kernel.
//!
//! These types describe model-execution semantics that are useful across providers. They are
//! intentionally narrower than Codex's existing Responses-shaped `Prompt`, `ResponseItem`, and
//! `ResponseEvent` types. Provider/product metadata and provider-private continuation state stay
//! below the model-runtime boundary and must not be added here merely to preserve one wire format.
//!
//! C1 introduced these definitions without changing production execution. C2 routes representable
//! regular sampling requests through `ModelRequest` while unsupported Codex/Responses-only shapes
//! remain on an explicit migration fallback. C3 begins moving the stream consumer to canonical
//! `ModelEvent` semantics while keeping unsupported/product events on a compatibility side channel.

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
    /// Optional assistant-message lifecycle phase. Providers that do not expose this distinction
    /// leave it as `None`.
    pub phase: Option<ModelMessagePhase>,
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

/// Harness-significant lifecycle phase for assistant text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelMessagePhase {
    /// Interim assistant text that may be followed by more model/tool activity in the same turn.
    Commentary,
    /// Terminal assistant answer for the current turn.
    Final,
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
    Bytes { media_type: String, data: Arc<[u8]> },
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
        availability: ModelToolAvailability,
        purpose: ModelToolPurpose,
    },
    /// Free-form textual tool input, optionally constrained by a grammar.
    Freeform {
        namespace: Option<String>,
        name: String,
        description: String,
        input_format: ModelFreeformInputFormat,
        availability: ModelToolAvailability,
        purpose: ModelToolPurpose,
    },
}

/// When a tool becomes visible to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelToolAvailability {
    /// The tool is present in the current model-visible tool surface.
    Immediate,
    /// The tool is intentionally withheld until model-driven discovery exposes it.
    Deferred,
}

/// Semantic role of a model-visible tool declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelToolPurpose {
    /// Normal harness capability invoked for its declared effect or result.
    Invocation,
    /// Harness capability whose invocation discovers or exposes additional tools.
    Discovery,
}

/// Input contract for a free-form tool without assuming a provider-specific wire shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFreeformInputFormat {
    /// Unconstrained free-form text.
    Text,
    /// Text constrained by a grammar understood by the target model backend.
    Grammar { syntax: String, definition: String },
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

/// Tool-call input category known before a streamed input value is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelToolInputKind {
    Json,
    Text,
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
        schema: Value,
        strict: bool,
    },
}

/// Stable correlation identifier for one streamed model output item.
///
/// The identifier is a model-runtime correlation key, not a provider response/item identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelItemId(pub String);

/// Partial header emitted when a streamed output item begins.
///
/// A started item must not require values that can only be known after deltas have completed. In
/// particular, structured tool input may be incomplete JSON at this point.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelOutputItemStart {
    Message {
        id: ModelItemId,
        phase: Option<ModelMessagePhase>,
    },
    ToolCall {
        id: ModelItemId,
        call_id: ModelToolCallId,
        namespace: Option<String>,
        name: String,
        input_kind: ModelToolInputKind,
    },
    Reasoning {
        id: ModelItemId,
    },
}

/// Provider-neutral completed model output item.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelOutputItem {
    Message {
        id: ModelItemId,
        phase: Option<ModelMessagePhase>,
        content: Vec<ModelContent>,
    },
    ToolCall {
        id: ModelItemId,
        call: ModelToolCall,
    },
    Reasoning {
        id: ModelItemId,
        summary: Vec<String>,
        /// Plaintext reasoning content when the backend exposes it. Opaque/encrypted continuation
        /// state remains adapter-private and is not represented here.
        content: Vec<String>,
    },
}

/// One event emitted by a model execution stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    Started,
    OutputItemStarted(ModelOutputItemStart),
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
    /// Begins a new logical reasoning section. This is harness-significant stream structure rather
    /// than a provider-specific summary-part wire event.
    ReasoningSectionStarted {
        item_id: ModelItemId,
        kind: ModelReasoningDeltaKind,
        section_index: u32,
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
