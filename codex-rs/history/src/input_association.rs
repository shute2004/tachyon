use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::InputIdentity;

/// Descriptive source association for an original input history record.
///
/// This value is persisted data, not proof of admission, human origin, authorization, persistence,
/// or transcript completeness. `Synthetic` is appropriate only when the caller knows the input
/// was produced internally; deserializing this value does not establish that fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
pub struct InputAssociation {
    /// Thread-local input-stream identity associated with the original record.
    pub identity: InputIdentity,
    /// Descriptive source classification for the associated input.
    pub source: InputSource,
}

/// The source classification recorded beside an input identity.
///
/// `Unknown` means no supported origin classification is known. `Synthetic` is reserved for
/// inputs a caller has established were produced internally; neither variant is an authority or
/// trust claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Unknown,
    Synthetic,
}
