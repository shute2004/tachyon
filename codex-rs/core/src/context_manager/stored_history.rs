use std::borrow::Borrow;

use codex_history::HistoryItem;
use codex_history::HistoryItemProjection;
use codex_history::HistoryMessageRole;
use codex_history::HistoryProjectionFallback;
use codex_history::ResponseItemEnvelope;
use codex_history::project_response_item;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::InterAgentCommunication;

use crate::context::is_user_authorization_message;
use crate::context_manager::history::is_user_turn_boundary;
use crate::event_mapping::is_contextual_user_message_content;

/// One lossless history entry with its canonical projection when available.
///
/// `HistoryItemProjection` owns the exact compatibility envelope in either variant. Keeping that
/// projection private prevents callers from mutating a wire-only sidecar without refreshing the
/// canonical value, while the compatibility view remains borrowable for the migration-era callers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoredHistoryEntry {
    projection: HistoryItemProjection,
}

impl StoredHistoryEntry {
    pub(crate) fn new(source: ResponseItemEnvelope) -> Self {
        Self {
            projection: project_response_item(source),
        }
    }

    pub(crate) fn canonical(&self) -> Option<&HistoryItem> {
        match &self.projection {
            HistoryItemProjection::Canonical { item, .. } => Some(item),
            HistoryItemProjection::Fallback { .. } => None,
        }
    }

    pub(crate) fn fallback(&self) -> Option<HistoryProjectionFallback> {
        match &self.projection {
            HistoryItemProjection::Canonical { .. } => None,
            HistoryItemProjection::Fallback { reason, .. } => Some(*reason),
        }
    }

    /// Borrow the exact Responses-shaped source retained by this entry.
    pub(crate) fn responses_compatibility(&self) -> &ResponseItemEnvelope {
        match &self.projection {
            HistoryItemProjection::Canonical { compatibility, .. }
            | HistoryItemProjection::Fallback { compatibility, .. } => compatibility,
        }
    }

    /// Consume this entry into its exact Responses-shaped source.
    pub(crate) fn into_responses_compatibility(self) -> ResponseItemEnvelope {
        match self.projection {
            HistoryItemProjection::Canonical { compatibility, .. }
            | HistoryItemProjection::Fallback { compatibility, .. } => compatibility,
        }
    }

    /// Consume this entry, rewrite its complete source, and refresh its projection.
    pub(crate) fn map_responses_compatibility(
        self,
        update: impl FnOnce(&mut ResponseItemEnvelope) -> bool,
    ) -> Option<Self> {
        let mut source = self.into_responses_compatibility();
        update(&mut source).then(|| Self::new(source))
    }

    pub(crate) fn is_user_turn_boundary(&self) -> bool {
        let Some(item) = self.canonical() else {
            debug_assert!(self.fallback().is_some());
            return is_user_turn_boundary(&self.responses_compatibility().item);
        };

        match item {
            HistoryItem::Message(message) => match message.role {
                HistoryMessageRole::User => matches!(
                    &self.responses_compatibility().item,
                    ResponseItem::Message { content, .. }
                        if !is_contextual_user_message_content(content)
                ),
                HistoryMessageRole::Assistant => matches!(
                    &self.responses_compatibility().item,
                    ResponseItem::Message { content, .. }
                        if InterAgentCommunication::is_message_content(content)
                ),
                HistoryMessageRole::System | HistoryMessageRole::Developer => false,
            },
            HistoryItem::Reasoning(_) | HistoryItem::ToolCall(_) | HistoryItem::ToolResult(_) => {
                false
            }
        }
    }

    pub(crate) fn is_user_authorization_message(&self) -> bool {
        match self.canonical() {
            Some(HistoryItem::Message(message)) if message.role == HistoryMessageRole::User => {
                is_user_authorization_message(&self.responses_compatibility().item)
            }
            Some(_) => false,
            None => is_user_authorization_message(&self.responses_compatibility().item),
        }
    }
}

impl Borrow<ResponseItem> for StoredHistoryEntry {
    fn borrow(&self) -> &ResponseItem {
        &self.responses_compatibility().item
    }
}
