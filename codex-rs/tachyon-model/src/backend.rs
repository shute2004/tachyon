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
use crate::ModelRequest;
use crate::route::ModelProviderId;
use crate::route::ModelRoute;

/// Boxed asynchronous operation returned by a model backend.
pub type ModelBackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Pinned source of ordered canonical model events.
pub type ModelEventStream = Pin<Box<dyn ModelEventSource>>;

/// Session-scoped factory for provider-bound, turn-scoped execution handles.
pub trait ModelBackend: std::fmt::Debug + Send + Sync {
    /// Start a fresh harness-turn handle bound to `provider_id`.
    ///
    /// The returned handle's route must carry this provider identity. Implementations may keep
    /// reusable provider-private resources behind the factory, but turn-affinity state must be
    /// fresh for every call.
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend>;

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
