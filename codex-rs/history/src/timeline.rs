use std::sync::Arc;

/// Descriptive classification supplied by the history owner when appending an item.
///
/// `UserInput` advances the user-message revision, but is not trusted input admission or a
/// security decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAppendKind {
    Ordinary,
    UserInput,
}

/// Meaning of a complete history replacement for revision tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryReplacement {
    /// Compaction changes model-visible history without changing user-input revision.
    Compaction,
    /// Reset/rollback replaces history and advances both revisions.
    Reset,
}

/// Chronological history storage with copy-on-write snapshots and revision ownership.
///
/// The timeline stores caller-owned values only. It does not interpret item semantics, decide
/// whether input is trusted, or define persistence encoding.
#[derive(Debug)]
pub struct HistoryTimeline<T> {
    items: Arc<Vec<T>>,
    history_version: u64,
    user_message_revision: u64,
}

impl<T> Clone for HistoryTimeline<T> {
    fn clone(&self) -> Self {
        Self {
            items: Arc::clone(&self.items),
            history_version: self.history_version,
            user_message_revision: self.user_message_revision,
        }
    }
}

impl<T> Default for HistoryTimeline<T> {
    fn default() -> Self {
        Self {
            items: Arc::new(Vec::new()),
            history_version: 0,
            user_message_revision: 0,
        }
    }
}

impl<T> HistoryTimeline<T> {
    /// Chronological items, oldest first.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// History generation, advanced by complete replacement rather than ordinary appends.
    pub fn history_version(&self) -> u64 {
        self.history_version
    }

    /// User-input/reset revision, independent of compaction generation.
    pub fn user_message_revision(&self) -> u64 {
        self.user_message_revision
    }

    /// Append one item, applying the caller's descriptive revision classification.
    pub fn record(&mut self, item: T, kind: HistoryAppendKind)
    where
        T: Clone,
    {
        Arc::make_mut(&mut self.items).push(item);
        if kind == HistoryAppendKind::UserInput {
            self.user_message_revision = self.user_message_revision.saturating_add(1);
        }
    }

    /// Replace the full timeline and advance revisions according to the replacement reason.
    pub fn replace(&mut self, items: Vec<T>, kind: HistoryReplacement) {
        self.items = Arc::new(items);
        self.history_version = self.history_version.saturating_add(1);
        if kind == HistoryReplacement::Reset {
            self.user_message_revision = self.user_message_revision.saturating_add(1);
        }
    }

    /// Update stored items without classifying the change as a history replacement.
    ///
    /// This is for normalization and paired-removal paths that transform existing records. It
    /// does not represent a reset or compaction and makes no input-trust or authentication
    /// decision.
    pub fn update_items(&mut self, items: Vec<T>) {
        self.items = Arc::new(items);
    }

    /// Mutable access for existing normalization/removal paths.
    ///
    /// Mutating through this lane does not automatically alter either revision.
    pub fn items_mut(&mut self) -> &mut Vec<T>
    where
        T: Clone,
    {
        Arc::make_mut(&mut self.items)
    }

    /// Consume the timeline, cloning the items only when another snapshot still shares them.
    pub fn into_items(self) -> Vec<T>
    where
        T: Clone,
    {
        Arc::unwrap_or_clone(self.items)
    }
}
