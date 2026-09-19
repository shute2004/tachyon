use crate::HistoryItem;
use crate::HistoryProjectionFallback;

/// One item in a borrowed, provider-neutral conversation-history snapshot.
///
/// The iterator that yields this view retains the source history order. A canonical item borrows
/// the stored semantic value without copying it; a fallback is explicit and carries only the
/// stable reason that the source item was not representable in the current vocabulary. Provider
/// payloads and compatibility envelopes are intentionally absent from this view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HistorySnapshotItemRef<'a> {
    /// A source item represented by the provider-neutral history vocabulary.
    Canonical(&'a HistoryItem),
    /// A source item retained on the compatibility path for the given projection reason.
    Fallback(HistoryProjectionFallback),
}
