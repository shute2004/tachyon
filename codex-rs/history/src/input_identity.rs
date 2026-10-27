use std::fmt::Display;
use std::fmt::Formatter;
use std::num::NonZeroU64;

use codex_protocol::ThreadId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Opaque identity for one thread-local input-stream incarnation.
///
/// Its UUID representation is identity only. It is not an ordering token; callers must not infer
/// creation order from it or compare sequence values across incarnations.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(transparent)]
pub struct InputStreamIncarnation(ThreadId);

impl InputStreamIncarnation {
    /// Creates a fresh opaque incarnation identity.
    pub fn new() -> Self {
        Self(ThreadId::new())
    }
}

impl Default for InputStreamIncarnation {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for InputStreamIncarnation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

/// Identity value for one position in a thread-local input stream.
///
/// A caller may attach this value to a reservation or original source record, but the value is
/// forgeable data, not proof of admission, human origin, authorization, persistence, or complete
/// transcript evidence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, JsonSchema, Serialize)]
pub struct InputIdentity {
    /// Logical thread that owns this input stream.
    pub thread_id: ThreadId,
    /// Stream incarnation used to distinguish sequence resets.
    pub incarnation: InputStreamIncarnation,
    /// Non-zero sequence within this incarnation.
    ///
    /// Compare sequences only when both the logical thread and incarnation are equal. Gaps are
    /// valid because reservations can go unused.
    pub sequence: NonZeroU64,
}
