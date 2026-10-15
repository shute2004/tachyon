use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use codex_exec_server_protocol::RequestId;

use super::PendingRequest;

pub(super) type PendingRequests = HashMap<RequestId, PendingRequest>;

pub(super) fn lock_pending(pending: &StdMutex<PendingRequests>) -> MutexGuard<'_, PendingRequests> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes a call registration if its future exits before a response arrives.
pub(super) struct PendingRequestGuard {
    pending: Arc<StdMutex<PendingRequests>>,
    request_id: RequestId,
}

impl PendingRequestGuard {
    pub(super) fn new(pending: Arc<StdMutex<PendingRequests>>, request_id: RequestId) -> Self {
        Self {
            pending,
            request_id,
        }
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        drop(lock_pending(&self.pending).remove(&self.request_id));
    }
}
