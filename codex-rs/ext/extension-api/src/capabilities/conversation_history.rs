use codex_history::HistorySnapshotItemRef;
use codex_protocol::models::ResponseItem;

/// Read-only conversation-history snapshot supplied by the extension host.
///
/// Implementations should retain the host's existing snapshot storage rather than
/// copying response payloads into an extension-owned collection.
pub trait ConversationHistorySnapshot: Send + Sync {
    /// Returns the generation of the history captured by this snapshot.
    fn history_version(&self) -> u64;

    /// Host-owned revision captured with this snapshot. Advances on user messages and
    /// history resets, but stays unchanged for compaction and internal context.
    fn user_message_revision(&self) -> u64;

    /// Returns the snapshot's provider-neutral items in conversation order.
    ///
    /// Canonical items are borrowed from the host-owned snapshot. Items that cannot yet be
    /// represented without losing source semantics are emitted as explicit fallbacks.
    fn items(&self) -> Box<dyn Iterator<Item = HistorySnapshotItemRef<'_>> + Send + '_>;

    /// Returns the snapshot's legacy Responses items in conversation order.
    ///
    /// This is a temporary migration-only compatibility surface for Guardian and other callers
    /// that still require raw Responses behavior. It must be removed after those consumers
    /// migrate to [`Self::items`].
    fn responses_compatibility_items(&self) -> Box<dyn Iterator<Item = &ResponseItem> + Send + '_>;
}
