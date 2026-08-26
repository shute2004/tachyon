//! Provider-neutral model route-selection vocabulary.
//!
//! D1 intentionally starts with the route properties that already make a real execution decision:
//! protocol identity and transport. Provider identity, endpoint resolution, authentication, and
//! provider-private runtime state remain below the Codex adapter until their ownership can be
//! separated without promoting `ModelProviderInfo` into Tachyon's generic contract.
//!
//! Protocol identity is opaque to the kernel. Concrete protocol identifiers such as
//! `openai.responses` belong to adapters rather than becoming enum variants in this module.

/// Opaque identity for one model wire protocol.
///
/// The kernel can compare and carry protocol identity without assuming that provider identity and
/// protocol identity are the same concept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelProtocol {
    id: String,
}

impl ModelProtocol {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Transport used to execute a model route.
///
/// These are transport mechanics, not provider or protocol identities. A protocol may support more
/// than one transport and a transport may be shared by multiple protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTransport {
    Http,
    WebSocket,
}

/// Current model execution route selection.
///
/// D1 contains only the pieces already needed for production dispatch. Provider, endpoint, auth,
/// and provider-private runtime state will be added as those existing responsibilities are
/// extracted from the Codex adapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRoute {
    protocol: ModelProtocol,
    transport: ModelTransport,
}

impl ModelRoute {
    pub fn new(protocol: ModelProtocol, transport: ModelTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    pub fn protocol(&self) -> &ModelProtocol {
        &self.protocol
    }

    pub fn transport(&self) -> ModelTransport {
        self.transport
    }
}
