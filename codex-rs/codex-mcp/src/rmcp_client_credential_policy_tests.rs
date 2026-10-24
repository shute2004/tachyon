use super::*;
use crate::server::McpCredentialPolicy;
use codex_config::McpServerTransportConfig;
use codex_config::types::AuthKeyringBackendKind;
use codex_config::types::OAuthCredentialsStoreMode;
use codex_exec_server::EnvironmentManager;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const REMOTE_ENVIRONMENT_ID: &str = "executor-test";
const SENTINEL_ENVIRONMENT_ERROR: &str = "sentinel environment resolution";
const EXECUTOR_TRANSPORT_ERROR: &str =
    "requires a remote HTTP transport without host environment headers or helpers";

fn http_config(environment_id: &str) -> McpServerConfig {
    McpServerConfig {
        auth: McpServerAuth::OAuth,
        transport: McpServerTransportConfig::StreamableHttp {
            url: "https://mcp.invalid/server".to_string(),
            bearer_token_env_var: None,
            http_headers: None,
            env_http_headers: None,
            http_headers_helper: None,
        },
        environment_id: environment_id.to_string(),
        enabled: true,
        required: false,
        supports_parallel_tool_calls: false,
        omit_tools_from: None,
        disabled_reason: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: None,
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: HashMap::new(),
    }
}

fn executor_server(config: McpServerConfig) -> EffectiveMcpServer {
    EffectiveMcpServer::from_config_with_policy(config, McpCredentialPolicy::ExecutorOnly)
}

async fn make_executor_client(config: McpServerConfig) -> Result<RmcpClient, StartupOutcomeError> {
    make_rmcp_client(
        "executor-owned",
        executor_server(config),
        OAuthCredentialsStoreMode::default(),
        AuthKeyringBackendKind::default(),
        McpRuntimeContext::new(
            Arc::new(EnvironmentManager::default_for_tests()),
            PathBuf::new(),
        ),
        Err(SENTINEL_ENVIRONMENT_ERROR.to_string()),
        None,
        McpProtocolMode::Legacy,
    )
    .await
}

fn assert_executor_transport_rejected(error: StartupOutcomeError) {
    match error {
        StartupOutcomeError::Failed { error, .. } => {
            assert!(error.contains(EXECUTOR_TRANSPORT_ERROR), "{error}");
        }
        StartupOutcomeError::Cancelled => panic!("expected transport-policy rejection"),
    }
}

#[test]
fn executor_only_bearer_resolution_rejects_present_and_missing_host_variables() {
    for env_var in ["PATH", "CODEX_TEST_UNSET_EXECUTOR_BEARER"] {
        let error = resolve_bearer_token(
            "executor-owned",
            Some(env_var),
            McpCredentialPolicy::ExecutorOnly,
        )
        .expect_err("ExecutorOnly must reject host fallback before reading the environment");
        assert!(
            error.to_string().contains("host fallback is disabled"),
            "{env_var}: {error}"
        );
    }
}

#[test]
fn no_bearer_environment_variable_needs_no_host_fallback_for_either_policy() {
    for policy in [
        McpCredentialPolicy::HostFallbackAllowed,
        McpCredentialPolicy::ExecutorOnly,
    ] {
        assert_eq!(
            resolve_bearer_token("policy-test", None, policy)
                .expect("no bearer environment variable should be accepted"),
            None
        );
    }
}

#[test]
fn host_fallback_allowed_preserves_existing_path_bearer_resolution() {
    match std::env::var("PATH") {
        Ok(expected) if !expected.is_empty() => {
            let resolved = resolve_bearer_token(
                "host-owned",
                Some("PATH"),
                McpCredentialPolicy::HostFallbackAllowed,
            )
            .expect("HostFallbackAllowed should resolve a configured PATH value")
            .expect("a nonempty PATH should produce a bearer value");
            assert!(
                resolved == expected,
                "HostFallbackAllowed did not resolve the configured PATH value"
            );
        }
        Ok(_) => {
            let error = resolve_bearer_token(
                "host-owned",
                Some("PATH"),
                McpCredentialPolicy::HostFallbackAllowed,
            )
            .expect_err("an empty PATH should preserve the existing error");
            assert!(error.to_string().contains("is empty"));
        }
        Err(std::env::VarError::NotPresent) => {
            let error = resolve_bearer_token(
                "host-owned",
                Some("PATH"),
                McpCredentialPolicy::HostFallbackAllowed,
            )
            .expect_err("a missing PATH should preserve the existing error");
            assert!(error.to_string().contains("is not set"));
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            let error = resolve_bearer_token(
                "host-owned",
                Some("PATH"),
                McpCredentialPolicy::HostFallbackAllowed,
            )
            .expect_err("a non-Unicode PATH should preserve the existing error");
            assert!(error.to_string().contains("contains invalid Unicode"));
        }
    }
}

#[tokio::test]
async fn executor_only_rejects_local_http_and_stdio_before_runtime_resolution() {
    let local_http = http_config(codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID);
    let local_stdio = McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: "must-not-run".to_string(),
            args: Vec::new(),
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        },
        ..http_config(codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID)
    };
    let remote_stdio = McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: "must-not-run".to_string(),
            args: Vec::new(),
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        },
        ..http_config(REMOTE_ENVIRONMENT_ID)
    };

    for config in [local_http, local_stdio, remote_stdio] {
        let error = match make_executor_client(config).await {
            Err(error) => error,
            Ok(_) => panic!("ExecutorOnly local and stdio transports must be rejected"),
        };
        assert_executor_transport_rejected(error);
    }
}

#[tokio::test]
async fn executor_only_rejects_host_environment_headers_and_helpers_before_resolution() {
    let mut env_header_config = http_config(REMOTE_ENVIRONMENT_ID);
    let McpServerTransportConfig::StreamableHttp {
        env_http_headers, ..
    } = &mut env_header_config.transport
    else {
        unreachable!("HTTP config helper should create streamable HTTP transport");
    };
    *env_http_headers = Some(HashMap::from([(
        "Authorization".to_string(),
        "CODEX_TEST_SENTINEL_NOT_A_SECRET".to_string(),
    )]));

    let mut helper_config = http_config(REMOTE_ENVIRONMENT_ID);
    let McpServerTransportConfig::StreamableHttp {
        http_headers_helper,
        ..
    } = &mut helper_config.transport
    else {
        unreachable!("HTTP config helper should create streamable HTTP transport");
    };
    *http_headers_helper = Some("must-not-be-invoked".to_string());

    for config in [env_header_config, helper_config] {
        let error = match make_executor_client(config).await {
            Err(error) => error,
            Ok(_) => panic!("ExecutorOnly host header mechanisms must be rejected"),
        };
        assert_executor_transport_rejected(error);
    }
}

#[tokio::test]
async fn executor_only_remote_http_with_literal_headers_passes_transport_guard() {
    let mut config = http_config(REMOTE_ENVIRONMENT_ID);
    let McpServerTransportConfig::StreamableHttp { http_headers, .. } = &mut config.transport
    else {
        unreachable!("HTTP config helper should create streamable HTTP transport");
    };
    *http_headers = Some(HashMap::from([(
        "X-Executor-Test".to_string(),
        "literal-value".to_string(),
    )]));

    let mut empty_env_headers_config = http_config(REMOTE_ENVIRONMENT_ID);
    let McpServerTransportConfig::StreamableHttp {
        env_http_headers, ..
    } = &mut empty_env_headers_config.transport
    else {
        unreachable!("HTTP config helper should create streamable HTTP transport");
    };
    *env_http_headers = Some(HashMap::new());

    for config in [config, empty_env_headers_config] {
        match make_executor_client(config).await {
            Err(StartupOutcomeError::Failed { error, .. }) => {
                assert_eq!(error, SENTINEL_ENVIRONMENT_ERROR);
            }
            Err(StartupOutcomeError::Cancelled) => {
                panic!("safe remote HTTP should pass the transport policy")
            }
            Ok(_) => panic!("sentinel environment failure should stop before transport startup"),
        }
    }
}
