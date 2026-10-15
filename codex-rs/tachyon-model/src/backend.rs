//! Provider-neutral model backend execution contract.
//!
//! A [`ModelBackend`] is session-scoped and creates a fresh, provider-bound
//! [`ModelTurnBackend`] for each harness turn. The same turn handle is intended to serve that
//! turn's sampling, tool follow-ups, and retries. Backend implementations may reuse opaque
//! provider-private resources behind the factory, but must not leak turn-affinity state between
//! handles.
//!
//! Requests and events crossing this boundary use only the canonical [`ModelRequest`] and
//! [`ModelEvent`] vocabulary. Model selection is a separate argument from provider identity. This
//! contract deliberately exposes no endpoint or authentication types, `ModelProviderInfo`, Codex
//! `Prompt` or `ResponseEvent`, or provider-private execution state.

use std::collections::HashSet;
use std::error::Error;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::{self};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use crate::ModelEvent;
use crate::ModelInputItem;
use crate::ModelRequest;
use crate::ModelUsage;
use crate::route::ModelProviderId;
use crate::route::ModelRoute;

/// Boxed asynchronous operation returned by a model backend.
pub type ModelBackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Pinned source of ordered canonical model events.
pub type ModelEventStream = Pin<Box<dyn ModelEventSource>>;

/// Result of one backend-managed compaction operation.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCompactionResult {
    /// Replacement conversation history in the order it should be installed.
    pub history: Vec<ModelCompactionItem>,
    /// Provider-neutral usage reported for the compaction request, when available.
    pub usage: Option<ModelUsage>,
}

/// One item in a backend-managed compaction result.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelCompactionItem {
    /// Retain an item from the exact normalized `ModelRequest::input` snapshot sent to compact.
    ///
    /// This is a request-slot reference, not a provider item ID. The caller remains responsible
    /// for validating attached item groups and harness context/provenance before installation.
    RetainedInput { index: usize },
    /// A newly produced replacement-history item with no claim to original input identity.
    NewItem(ModelInputItem),
}

impl ModelCompactionResult {
    /// Validates only that retained-input references are in range and unique.
    ///
    /// This does not validate usage, attached item groups, or harness context/provenance. Callers
    /// must validate and prepare the complete replacement before installing any part of it.
    /// Empty history is permitted by this reference-only validator.
    pub fn validate_retained_inputs(&self, input_len: usize) -> Result<(), ModelBackendError> {
        let mut retained_inputs = HashSet::new();
        for (output_index, item) in self.history.iter().enumerate() {
            let ModelCompactionItem::RetainedInput { index } = item else {
                continue;
            };
            if *index >= input_len {
                return Err(ModelBackendError::Failed {
                    message: format!(
                        "compaction history item {output_index} references input index {index}, but request input length is {input_len}"
                    ),
                    retryable: false,
                });
            }
            if !retained_inputs.insert(*index) {
                return Err(ModelBackendError::Failed {
                    message: format!(
                        "compaction history item {output_index} references input index {index} more than once"
                    ),
                    retryable: false,
                });
            }
        }
        Ok(())
    }
}

/// Session-scoped factory for provider-bound, turn-scoped execution handles.
pub trait ModelBackend: std::fmt::Debug + Send + Sync {
    /// Start a fresh harness-turn handle bound to `provider_id`.
    ///
    /// The returned handle's route must carry this provider identity. Implementations may keep
    /// reusable provider-private resources behind the factory, but turn-affinity state must be
    /// fresh for every call.
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend>;

    /// Reports whether this backend opts into backend-managed model compaction.
    ///
    /// This is an explicit backend capability, not a property inferred from a provider, protocol,
    /// endpoint, or transport. Backends that do not implement compaction should keep the default.
    fn supports_compaction(&self) -> bool {
        false
    }

    /// Creates an independent session-scoped backend for a delegated child session.
    ///
    /// The returned backend owns independent session-affinity and preparation state. An
    /// implementation may reuse private transport or authentication resources behind that new
    /// factory, but must not share the parent factory itself as the child's session runtime. The
    /// returned backend still creates a fresh turn handle for every call to [`Self::begin_turn`].
    ///
    /// Backends that cannot create an independent child session return
    /// [`ModelBackendError::UnsupportedRequest`]. This is reported to the caller and does not
    /// authorize silently falling back to or sharing the parent runtime; a caller may explicitly
    /// supply another child backend when appropriate.
    fn new_child_session(
        &self,
    ) -> ModelBackendFuture<'_, Result<Arc<dyn ModelBackend>, ModelBackendError>> {
        Box::pin(std::future::ready(Err(
            ModelBackendError::UnsupportedRequest(
                "independent child sessions are not supported by this backend".to_string(),
            ),
        )))
    }

    /// Optionally prepare session-scoped resources before the first turn.
    ///
    /// Backends without preparation work may keep this default no-op implementation.
    fn prepare_session(&self) -> ModelBackendFuture<'_, Result<(), ModelBackendError>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Provider-bound execution handle owned by one harness turn.
///
/// Reuse one handle for sampling, tool follow-ups, and retries that belong to the same turn.
pub trait ModelTurnBackend: Send + Sync {
    /// The fully identified route used by this turn.
    fn route(&self) -> &ModelRoute;

    /// Optionally recover this same turn handle after a retryable stream error.
    ///
    /// Returning `Some(warning)` means the backend made a backend-private, non-blocking request-path
    /// change and the same handle is ready for one more retry. Concrete endpoint, transport,
    /// authentication, and provider wire details remain private to the backend and must not be
    /// surfaced through this contract. Implementations must keep recovery finite for each eligible
    /// failure mode and return `None` after their recovery attempts are exhausted. Recovery must not
    /// change the selected model or configured provider identity.
    fn try_recover_after_stream_error(&mut self) -> Option<String> {
        None
    }

    /// Execute one canonical request with an explicitly selected model identity.
    ///
    /// The selected `model_id` is distinct from the provider identity in [`Self::route`]. The
    /// returned stream yields canonical events in order, including a [`ModelEvent::Completed`]
    /// before ending normally. An interrupted operation that should be retried must report an
    /// explicit retryable error. The stream exposes no provider wire events or private continuation
    /// state.
    fn stream<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>>;

    /// Compacts the supplied canonical request using backend-managed behavior.
    ///
    /// Callers should route here only when the session-scoped backend opted in through
    /// [`ModelBackend::supports_compaction`]. The operation remains on this provider-bound turn
    /// handle. Retained history entries must refer to the exact normalized `request.input` slots;
    /// newly produced entries must use [`ModelCompactionItem::NewItem`].
    fn compact<'a>(
        &'a mut self,
        _model_id: &'a str,
        _request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelCompactionResult, ModelBackendError>> {
        Box::pin(std::future::ready(Err(
            ModelBackendError::UnsupportedRequest(
                "backend-managed compaction is not supported by this backend".to_string(),
            ),
        )))
    }
}

/// Poll-based source for an ordered stream of canonical model events.
pub trait ModelEventSource: Send {
    /// Poll the next event, returning `None` when the stream is complete.
    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<ModelEvent, ModelBackendError>>>;
}

/// Provider-neutral failure returned by a model backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelBackendError {
    /// Execution failed; `retryable` indicates whether the backend considers retry appropriate.
    Failed { message: String, retryable: bool },
    /// The backend cannot execute the supplied canonical request.
    UnsupportedRequest(String),
    /// The operation was cancelled.
    Cancelled,
}

impl Display for ModelBackendError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed { message, .. } => formatter.write_str(message),
            Self::UnsupportedRequest(message) => {
                write!(formatter, "unsupported model request: {message}")
            }
            Self::Cancelled => formatter.write_str("model backend operation cancelled"),
        }
    }
}

impl Error for ModelBackendError {}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
