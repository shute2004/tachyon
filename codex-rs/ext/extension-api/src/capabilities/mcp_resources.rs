use std::any::Any;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_protocol::mcp::Resource;
use codex_protocol::mcp::ResourceContent;

/// Error returned by a host-provided MCP resource operation.
pub type McpResourceError = Box<dyn Error + Send + Sync + 'static>;

/// Future returned by a host-provided MCP resource operation.
pub type McpResourceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, McpResourceError>> + Send + 'a>>;

/// Future returned when checking whether an MCP server is available.
pub type McpResourceAvailabilityFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// One page of resources returned by an MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct McpResourcePage {
    pub resources: Vec<Resource>,
    pub next_cursor: Option<String>,
}

/// Contents returned after reading one MCP resource.
#[derive(Clone, Debug, PartialEq)]
pub struct McpResourceReadResult {
    pub contents: Vec<ResourceContent>,
}

trait McpResourceCacheIdentity: Any + Send + Sync {
    fn equals(&self, other: &dyn McpResourceCacheIdentity) -> bool;
    fn as_any(&self) -> &dyn Any;
}

struct TypedMcpResourceCacheIdentity<T>(T);

impl<T> McpResourceCacheIdentity for TypedMcpResourceCacheIdentity<T>
where
    T: Eq + Send + Sync + 'static,
{
    fn equals(&self, other: &dyn McpResourceCacheIdentity) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.0 == other.0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Opaque identity for one host MCP resource connection generation.
///
/// Hosts can wrap their native generation key without exposing its concrete
/// type. Extensions should use equality only to decide whether cached resource
/// data is still valid.
#[derive(Clone)]
pub struct McpResourceCacheKey(Arc<dyn McpResourceCacheIdentity>);

impl McpResourceCacheKey {
    pub fn new<T>(identity: T) -> Self
    where
        T: Eq + Send + Sync + 'static,
    {
        Self(Arc::new(TypedMcpResourceCacheIdentity(identity)))
    }
}

impl PartialEq for McpResourceCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.equals(other.0.as_ref())
    }
}

impl Eq for McpResourceCacheKey {}

impl fmt::Debug for McpResourceCacheKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("McpResourceCacheKey")
            .field(&"<opaque>")
            .finish()
    }
}

/// Read-only MCP resource capability supplied by the extension host.
pub trait McpResourceAccess: fmt::Debug + Send + Sync {
    /// Returns the identity of the connection generation used by this access object.
    fn cache_key(&self) -> McpResourceCacheKey;

    /// Returns whether this access object can address the named server.
    fn has_server<'a>(&'a self, server: &'a str) -> McpResourceAvailabilityFuture<'a>;

    /// Lists one resource page from the named server.
    fn list_resources<'a>(
        &'a self,
        server: &'a str,
        cursor: Option<String>,
    ) -> McpResourceFuture<'a, McpResourcePage>;

    /// Reads one resource from the named server.
    fn read_resource<'a>(
        &'a self,
        server: &'a str,
        uri: &'a str,
    ) -> McpResourceFuture<'a, McpResourceReadResult>;
}
