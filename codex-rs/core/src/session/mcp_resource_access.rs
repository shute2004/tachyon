use std::sync::Arc;

use codex_extension_api::McpResourceAccess;
use codex_extension_api::McpResourceAvailabilityFuture;
use codex_extension_api::McpResourceCacheKey;
use codex_extension_api::McpResourceFuture;
use codex_extension_api::McpResourcePage;
use codex_extension_api::McpResourceReadResult;
use codex_mcp::McpResourceClient;
use codex_mcp::McpRuntime;

/// Core-owned adapter that exposes MCP resource access without leaking the
/// concrete MCP runtime through the extension lifecycle API.
#[derive(Clone, Debug)]
pub(crate) struct McpResourceAccessAdapter {
    client: McpResourceClient,
}

impl McpResourceAccessAdapter {
    pub(crate) fn new(runtime: Arc<McpRuntime>) -> Self {
        Self {
            client: McpResourceClient::new(runtime),
        }
    }
}

impl McpResourceAccess for McpResourceAccessAdapter {
    fn cache_key(&self) -> McpResourceCacheKey {
        McpResourceCacheKey::new(self.client.cache_key())
    }

    fn has_server<'a>(&'a self, server: &'a str) -> McpResourceAvailabilityFuture<'a> {
        Box::pin(self.client.has_server(server))
    }

    fn list_resources<'a>(
        &'a self,
        server: &'a str,
        cursor: Option<String>,
    ) -> McpResourceFuture<'a, McpResourcePage> {
        Box::pin(async move {
            let page = self
                .client
                .list_resources(server, cursor)
                .await
                .map_err(anyhow::Error::into_boxed_dyn_error)?;
            Ok(McpResourcePage {
                resources: page.resources,
                next_cursor: page.next_cursor,
            })
        })
    }

    fn read_resource<'a>(
        &'a self,
        server: &'a str,
        uri: &'a str,
    ) -> McpResourceFuture<'a, McpResourceReadResult> {
        Box::pin(async move {
            let result = self
                .client
                .read_resource(server, uri)
                .await
                .map_err(anyhow::Error::into_boxed_dyn_error)?;
            Ok(McpResourceReadResult {
                contents: result.contents,
            })
        })
    }
}
