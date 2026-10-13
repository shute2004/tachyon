use std::borrow::Cow;

use codex_protocol::models::ResponseItem;

/// Transitional Codex-specific Responses compatibility view of conversation history.
///
/// The host's copy-on-write canonical history timeline remains the primary storage. Canonical
/// entries are reconstructed as owned `ResponseItem`s when read, while legacy entries remain
/// borrowed. This view must not become a persistent secondary mirror or cache.
pub trait ConversationHistorySnapshot: Send + Sync {
    /// Returns the generation of the history captured by this snapshot.
    fn history_version(&self) -> u64;

    /// Host-owned revision captured with this snapshot. Advances on user messages and
    /// history resets, but stays unchanged for compaction and internal context.
    fn user_message_revision(&self) -> u64;

    /// Returns compatibility response items in conversation order.
    fn items(&self) -> Box<dyn Iterator<Item = Cow<'_, ResponseItem>> + Send + '_>;
}
