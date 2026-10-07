//! Compatibility re-export for the existing model-runtime module path.
//!
//! The canonical model request/event vocabulary is owned by the Tachyon kernel in the
//! `tachyon-model` crate. These types describe model-execution semantics that are useful across
//! providers. They are intentionally narrower than Codex's existing Responses-shaped `Prompt`,
//! `ResponseItem`, and `ResponseEvent` types. Provider/product metadata and provider-private
//! continuation state stay below the model-runtime boundary and must not be added here merely to
//! preserve one wire format.
//!
//! C1 introduced these definitions without changing production execution. C2 routes representable
//! regular sampling requests through `ModelRequest` while unsupported Codex/Responses-only shapes
//! remain on an explicit migration fallback. C3 begins moving the stream consumer to canonical
//! `ModelEvent` semantics while keeping unsupported/product events on a compatibility side channel.

pub use tachyon_model::{
    ModelCompletion, ModelContent, ModelEvent, ModelFreeformInputFormat, ModelImageDetail,
    ModelInputItem, ModelItemId, ModelMediaSource, ModelMessage, ModelMessagePhase,
    ModelMessageRole, ModelOutputConfig, ModelOutputFormat, ModelOutputItem, ModelOutputItemStart,
    ModelReasoning, ModelReasoningDeltaKind, ModelRequest, ModelToolAvailability, ModelToolCall,
    ModelToolCallId, ModelToolInput, ModelToolInputKind, ModelToolPurpose, ModelToolResult,
    ModelToolResultContent, ModelToolSpec, ModelUsage,
};

#[cfg(test)]
#[path = "ir_tests.rs"]
mod tests;
