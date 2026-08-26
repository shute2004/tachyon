//! Provider-neutral model route-selection vocabulary.
//!
//! Route extraction is incremental. D1 introduced protocol identity and transport. D2 introduces
//! provider identity as a distinct route dimension without promoting `ModelProviderInfo` into
//! Tachyon's generic contract.
//!
//! The transitional Codex bridge does not currently retain the configured provider registry key at
//! the model-runtime construction boundary. It therefore uses a crate-private unresolved route
//! selection rather than fabricating an identity from display names, endpoints, or authentication
//! details or making missing provider identity part of Tachyon's public route contract.
//!
//! Provider and protocol identities are independent opaque values. A provider may expose multiple
//! protocols, and multiple providers may expose the same protocol. Concrete protocol identifiers
//! such as `openai.responses` belong to adapters rather than becoming enum variants here.

/// Stable opaque identity for a model provider.
///
/// This is an identity only. Provider configuration, credentials, endpoints, capabilities, and
/// provider-private runtime state do not belong in this value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelProviderId {
    id: String,
}

impl ModelProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

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

/// Fully identified model execution route.
///
/// Provider, protocol, and transport are independent dimensions. Endpoint resolution,
/// authentication, and provider-private runtime state will be added only as those existing
/// responsibilities are extracted from the Codex adapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRoute {
    provider_id: ModelProviderId,
    protocol: ModelProtocol,
    transport: ModelTransport,
}

impl ModelRoute {
    pub fn new(
        provider_id: ModelProviderId,
        protocol: ModelProtocol,
        transport: ModelTransport,
    ) -> Self {
        Self {
            provider_id,
            protocol,
            transport,
        }
    }

    pub fn provider_id(&self) -> &ModelProviderId {
        &self.provider_id
    }

    pub fn protocol(&self) -> &ModelProtocol {
        &self.protocol
    }

    pub fn transport(&self) -> ModelTransport {
        self.transport
    }
}

/// Migration-only route selection used while the Codex bridge has not yet retained its configured
/// provider registry identity at the model-runtime construction boundary.
///
/// Keeping this type crate-private prevents extraction state from becoming part of Tachyon's
/// provider-neutral route contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnresolvedModelRoute {
    protocol: ModelProtocol,
    transport: ModelTransport,
}

impl UnresolvedModelRoute {
    pub(crate) fn new(protocol: ModelProtocol, transport: ModelTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    pub(crate) fn protocol(&self) -> &ModelProtocol {
        &self.protocol
    }

    pub(crate) fn transport(&self) -> ModelTransport {
        self.transport
    }
}
