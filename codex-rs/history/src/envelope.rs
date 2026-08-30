use std::borrow::Borrow;
use std::ops::Deref;
use std::ops::DerefMut;

/// A persisted history item together with sidecar metadata.
///
/// Both the item and metadata representations are type parameters. This keeps the envelope itself
/// independent from Codex/Responses payloads and from migration-era host metadata. Existing
/// Responses history can therefore remain lossless while later slices introduce kernel-owned item
/// and metadata types behind the same envelope shape.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEnvelope<T, M> {
    pub item: T,
    pub metadata: Option<M>,
}

impl<T, M> HistoryEnvelope<T, M> {
    pub fn new(item: T) -> Self {
        Self {
            item,
            metadata: None,
        }
    }

    pub fn with_metadata(item: T, metadata: M) -> Self {
        Self {
            item,
            metadata: Some(metadata),
        }
    }

    pub fn into_item(self) -> T {
        self.item
    }
}

impl<T, M> From<T> for HistoryEnvelope<T, M> {
    fn from(item: T) -> Self {
        Self::new(item)
    }
}

impl<T, M> Deref for HistoryEnvelope<T, M> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<T, M> DerefMut for HistoryEnvelope<T, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl<T, M> Borrow<T> for HistoryEnvelope<T, M> {
    fn borrow(&self) -> &T {
        &self.item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestMetadata {
        budget: usize,
    }

    #[test]
    fn generic_history_envelope_preserves_non_provider_item_and_metadata() {
        let mut envelope =
            HistoryEnvelope::with_metadata(String::from("first"), TestMetadata { budget: 2048 });

        assert_eq!(envelope.as_str(), "first");
        *envelope = String::from("second");
        assert_eq!(envelope.metadata.as_ref().unwrap().budget, 2048);
        assert_eq!(envelope.into_item(), "second");
    }
}
