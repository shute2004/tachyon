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

pub use tachyon_model::ModelCompletion;
pub use tachyon_model::ModelContent;
pub use tachyon_model::ModelEvent;
pub use tachyon_model::ModelFreeformInputFormat;
pub use tachyon_model::ModelImageDetail;
pub use tachyon_model::ModelInputItem;
pub use tachyon_model::ModelItemId;
pub use tachyon_model::ModelMediaSource;
pub use tachyon_model::ModelMessage;
pub use tachyon_model::ModelMessagePhase;
pub use tachyon_model::ModelMessageRole;
pub use tachyon_model::ModelOutputConfig;
pub use tachyon_model::ModelOutputFormat;
pub use tachyon_model::ModelOutputItem;
pub use tachyon_model::ModelOutputItemStart;
pub use tachyon_model::ModelReasoning;
pub use tachyon_model::ModelReasoningDeltaKind;
pub use tachyon_model::ModelRequest;
pub use tachyon_model::ModelToolAvailability;
pub use tachyon_model::ModelToolCall;
pub use tachyon_model::ModelToolCallId;
pub use tachyon_model::ModelToolInput;
pub use tachyon_model::ModelToolInputKind;
pub use tachyon_model::ModelToolPurpose;
pub use tachyon_model::ModelToolResult;
pub use tachyon_model::ModelToolResultContent;
pub use tachyon_model::ModelToolSpec;
pub use tachyon_model::ModelUsage;

#[cfg(test)]
#[path = "ir_tests.rs"]
mod tests;
