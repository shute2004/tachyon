use std::borrow::Borrow;
use std::ops::Deref;
use std::ops::DerefMut;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// A persisted history item together with metadata owned by the harness.
///
/// The envelope is intentionally generic over the item representation. During the
/// history-neutralization migration, Codex/Responses items can remain as a compatibility payload
/// while new kernel-owned history item types adopt the same envelope without changing persistence
/// metadata semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEnvelope<T> {
    pub item: T,
    pub metadata: Option<HistoryMetadata>,
}

/// Harness-owned metadata that is persisted beside, rather than inside, a history item.
///
/// Fields in this sidecar must describe reusable harness behavior. Provider-private payload data
/// belongs in the provider compatibility representation, not in this metadata bag.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
pub struct HistoryMetadata {
    /// Whether a developer message was supplied by an app-server client.
    ///
    /// This remains a migration-era host hint and will be reconsidered when the public host
    /// boundary is neutralized. Keeping it here preserves the existing persisted shape for now.
    #[serde(default)]
    pub client_authored: bool,

    /// Overrides history's fallback truncation budget, including on resume.
    /// Measured in tokens, with any tool-specific allowance already included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_token_limit_override: Option<usize>,
}

impl<T> HistoryEnvelope<T> {
    pub fn new(item: T) -> Self {
        Self {
            item,
            metadata: None,
        }
    }

    pub fn with_metadata(item: T, metadata: HistoryMetadata) -> Self {
        Self {
            item,
            metadata: Some(metadata),
        }
    }

    pub fn into_item(self) -> T {
        self.item
    }
}

impl<T> From<T> for HistoryEnvelope<T> {
    fn from(item: T) -> Self {
        Self::new(item)
    }
}

impl<T> Deref for HistoryEnvelope<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T> DerefMut for HistoryEnvelope<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl<T> Borrow<T> for HistoryEnvelope<T> {
    fn borrow(&self) -> &T {
        &self.item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_history_envelope_preserves_non_provider_item() {
        let mut envelope = HistoryEnvelope::with_metadata(
            String::from("first"),
            HistoryMetadata {
                fallback_token_limit_override: Some(2048),
                ..Default::default()
            },
        );

        assert_eq!(envelope.as_str(), "first");
        *envelope = String::from("second");
        assert_eq!(envelope.metadata.as_ref().unwrap().fallback_token_limit_override, Some(2048));
        assert_eq!(envelope.into_item(), "second");
    }
}
