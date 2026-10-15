use super::EffectiveMcpServer;
use super::McpCredentialPolicy;
use super::McpServerConnectionIdentity;
use super::referenced_environment_variables;
use crate::McpProtocolMode;
use crate::McpRuntimeContext;
use crate::ToolInfo;
use crate::tool_catalog_cache::McpToolCatalogCache;
use codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;
use codex_config::McpServerConfig;
use codex_config::types::AuthKeyringBackendKind;
use codex_config::types::OAuthCredentialsStoreMode;
use codex_exec_server::Environment;
use codex_exec_server_test_support::environment_manager_without_environments;
use codex_protocol::mcp::ClientMcpExtensions;
use pretty_assertions::assert_eq;
use rmcp::model::ElicitationCapability;
use rmcp::model::JsonObject;
use rmcp::model::Tool;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn remote_http_connections_track_host_headers_but_not_executor_bearer_tokens() {
    let mut config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/mcp",
        "environment_id": "executor-1",
        "bearer_token_env_var": "NODE_REPL_AUTH_TOKEN",
        "env_http_headers": {"X-Api-Key": "PATH"},
    }))
    .expect("remote MCP configuration should deserialize");

    assert_eq!(
        referenced_environment_variables(&config),
        vec![("PATH".to_string(), std::env::var_os("PATH"))],
    );

    let remote_host_bearer: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/mcp",
        "environment_id": "executor-1",
        "bearer_token_env_var": "PATH",
    }))
    .expect("host-resolved remote MCP configuration should deserialize");
    assert_eq!(
        referenced_environment_variables(&remote_host_bearer),
        vec![("PATH".to_string(), std::env::var_os("PATH"))],
    );

    config.environment_id = DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string();
    assert_eq!(
        referenced_environment_variables(&config),
        vec![
            (
                "NODE_REPL_AUTH_TOKEN".to_string(),
                std::env::var_os("NODE_REPL_AUTH_TOKEN"),
            ),
            ("PATH".to_string(), std::env::var_os("PATH")),
        ],
    );
}

#[test]
fn executor_only_connections_skip_host_environment_credential_capture() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/mcp",
        "environment_id": "executor-1",
        "bearer_token_env_var": "PATH",
    }))
    .expect("MCP configuration should deserialize");
    let runtime_context = test_runtime_context();

    let host_identity = connection_identity(
        &config,
        McpCredentialPolicy::HostFallbackAllowed,
        &runtime_context,
    );
    assert!(std::env::var_os("PATH").is_some());
    assert_eq!(
        host_identity.referenced_environment_variables,
        vec![("PATH".to_string(), std::env::var_os("PATH"))],
    );

    let executor_identity =
        connection_identity(&config, McpCredentialPolicy::ExecutorOnly, &runtime_context);
    assert!(
        executor_identity
            .referenced_environment_variables
            .is_empty()
    );
    assert!(!host_identity.has_same_connection_config(&executor_identity));
}

#[test]
fn http_tool_catalog_cache_isolates_credential_policies_and_reuses_same_policy() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/mcp",
        "environment_id": "executor-1",
        "http_headers": {"Authorization": "Bearer test-canary"},
    }))
    .expect("MCP configuration should deserialize");
    let runtime_context = test_runtime_context();
    let host_identity = connection_identity(
        &config,
        McpCredentialPolicy::HostFallbackAllowed,
        &runtime_context,
    );
    let executor_identity =
        connection_identity(&config, McpCredentialPolicy::ExecutorOnly, &runtime_context);
    let same_policy_identity = connection_identity(
        &config,
        McpCredentialPolicy::HostFallbackAllowed,
        &runtime_context,
    );
    assert!(host_identity.has_same_connection_config(&same_policy_identity));
    assert!(!host_identity.has_same_connection_config(&executor_identity));

    let cache = McpToolCatalogCache::default();
    let context = |identity: &McpServerConnectionIdentity| {
        cache
            .context(
                "docs",
                &config,
                &runtime_context,
                /*resolved_environment*/ None,
                (
                    &ElicitationCapability::default(),
                    &ClientMcpExtensions::default(),
                ),
                Some((
                    identity,
                    McpProtocolMode::Legacy,
                    /*agent_plugin*/ false,
                )),
            )
            .expect("eligible HTTP configuration should have a cache context")
    };
    let host_context = context(&host_identity);
    let executor_context = context(&executor_identity);
    let same_policy_context = context(&same_policy_identity);

    let host_tool = test_tool("host_snapshot");
    host_context.publish_if_newest(host_context.begin_fetch(), &[host_tool]);
    assert_eq!(cached_tool_names(&host_context), vec!["host_snapshot"]);
    assert!(executor_context.current_tools().is_none());
    assert_eq!(
        cached_tool_names(&same_policy_context),
        vec!["host_snapshot"]
    );

    let executor_tool = test_tool("executor_snapshot");
    executor_context.publish_if_newest(executor_context.begin_fetch(), &[executor_tool]);
    assert_eq!(
        cached_tool_names(&executor_context),
        vec!["executor_snapshot"]
    );
    assert_eq!(cached_tool_names(&host_context), vec!["host_snapshot"]);
    assert_eq!(
        cached_tool_names(&same_policy_context),
        vec!["host_snapshot"]
    );
}

#[test]
fn executor_only_stdio_tool_catalog_cache_bypasses_host_environment_snapshot() {
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "docs-mcp",
        "environment_id": "executor-1",
        "env_vars": ["PATH"],
    }))
    .expect("MCP configuration should deserialize");
    let runtime_context = test_runtime_context();
    let host_identity = connection_identity(
        &config,
        McpCredentialPolicy::HostFallbackAllowed,
        &runtime_context,
    );
    let executor_identity =
        connection_identity(&config, McpCredentialPolicy::ExecutorOnly, &runtime_context);
    let same_policy_identity = connection_identity(
        &config,
        McpCredentialPolicy::HostFallbackAllowed,
        &runtime_context,
    );
    assert!(std::env::var_os("PATH").is_some());

    let cache = McpToolCatalogCache::default();
    let context = |identity: &McpServerConnectionIdentity| {
        cache.context(
            "docs",
            &config,
            &runtime_context,
            /*resolved_environment*/ None,
            (
                &ElicitationCapability::default(),
                &ClientMcpExtensions::default(),
            ),
            Some((
                identity,
                McpProtocolMode::Legacy,
                /*agent_plugin*/ false,
            )),
        )
    };
    let host_context = context(&host_identity).expect("host Stdio cache context");
    let same_policy_context = context(&same_policy_identity).expect("reused host cache context");

    host_context.publish_if_newest(host_context.begin_fetch(), &[test_tool("host_snapshot")]);
    assert_eq!(cached_tool_names(&host_context), vec!["host_snapshot"]);
    assert_eq!(
        cached_tool_names(&same_policy_context),
        vec!["host_snapshot"]
    );
    assert!(context(&executor_identity).is_none());
    assert_eq!(cached_tool_names(&host_context), vec!["host_snapshot"]);
}

fn connection_identity(
    config: &McpServerConfig,
    credential_policy: McpCredentialPolicy,
    runtime_context: &McpRuntimeContext,
) -> McpServerConnectionIdentity {
    let server = EffectiveMcpServer::from_config_with_policy(config.clone(), credential_policy);
    let resolved_environment: Result<Option<Arc<Environment>>, String> = Ok(None);
    McpServerConnectionIdentity::new(
        "docs",
        &server,
        OAuthCredentialsStoreMode::default(),
        AuthKeyringBackendKind::default(),
        &resolved_environment,
        runtime_context,
        /*runtime_auth_provider*/ None,
        /*auth*/ None,
        /*codex_apps_cache_identity*/ None,
        ElicitationCapability::default(),
        ClientMcpExtensions::default(),
        /*previous_identity*/ None,
    )
}

fn test_runtime_context() -> McpRuntimeContext {
    McpRuntimeContext::new(
        Arc::new(environment_manager_without_environments()),
        PathBuf::from("/tmp"),
    )
}

fn test_tool(name: &str) -> ToolInfo {
    ToolInfo {
        server_name: "docs".to_string(),
        supports_parallel_tool_calls: false,
        server_origin: None,
        callable_name: name.to_string(),
        callable_namespace: "docs".to_string(),
        namespace_description: None,
        tool: Tool::new(
            name.to_string(),
            format!("Test tool: {name}"),
            Arc::new(JsonObject::default()),
        ),
        openai_file_input_optional_fields: Default::default(),
        connector_id: None,
        connector_name: None,
        plugin_display_names: Vec::new(),
    }
}

fn cached_tool_names(
    context: &crate::tool_catalog_cache::McpToolCatalogCacheContext,
) -> Vec<String> {
    context
        .current_tools()
        .expect("published cache snapshot")
        .into_iter()
        .map(|tool| tool.callable_name)
        .collect()
}
