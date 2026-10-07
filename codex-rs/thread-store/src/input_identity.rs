use serde::Deserialize;
use serde::Serialize;

pub use codex_rollout::InputIdentity as ReservedInputIdentity;
pub use codex_rollout::InputStreamIncarnation;

/// Whether the journal has prior reserved values for the returned incarnation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum InputIdentityContinuity {
    /// This reservation starts a fresh incarnation with its own sequence stream.
    ///
    /// No ordering relationship with another incarnation is implied.
    NewIncarnation,
    /// The journal has previous reservations in the same incarnation.
    ///
    /// This does not guarantee that every earlier input was accepted or that the complete input
    /// transcript is present.
    Continuing,
}

/// Result of durably reserving a stream-local input identity.
///
/// The identity supports ordering only within one `(thread_id, incarnation)` stream. A
/// reservation may be unused, and continuity describes reserved values only—not accepted input,
/// approval, durable canonical evidence, or transcript completeness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct InputIdentityReservation {
    /// Reserved logical-thread, incarnation, and sequence identity.
    pub identity: ReservedInputIdentity,
    /// Whether this journal has prior reserved values for this incarnation.
    pub continuity: InputIdentityContinuity,
}
