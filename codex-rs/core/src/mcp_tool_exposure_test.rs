use std::collections::HashMap;
use std::sync::Arc;

use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_mcp::McpPluginAttribution;
use codex_mcp::McpServerRegistration;
use codex_mcp::ResolvedMcpCatalog;
use codex_mcp::ToolInfo;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use pretty_assertions::assert_eq;
use rmcp::model::Icon;
use rmcp::model::JsonObject;
use rmcp::model::MetaObject;
use rmcp::model::Tool;
use rmcp::model::ToolAnnotations;

use super::*;
use crate::config::CONFIG_TOML_FILE;
use crate::config::Config;
use crate::config::ConfigBuilder;
use crate::config::test_config;
use crate::tools::registry::CoreToolRuntime;
use tempfile::tempdir;

fn make_mcp_tool(
    server_name: &str,
    tool_name: &str,
    callable_namespace: &str,
    callable_name: &str,
    connector_id: Option<&str>,
    connector_name: Option<&str>,
) -> ToolInfo {
    ToolInfo {
        server_name: server_name.to_string(),
        supports_parallel_tool_calls: false,
        server_origin: None,
        callable_name: callable_name.to_string(),
        callable_namespace: callable_namespace.to_string(),
        namespace_description: None,
        tool: Tool::new(
            tool_name.to_string(),
            format!("Test tool: {tool_name}"),
            Arc::new(JsonObject::default()),
        ),
        openai_file_input_optional_fields: Default::default(),
        connector_id: connector_id.map(str::to_string),
        connector_name: connector_name.map(str::to_string),
        plugin_display_names: Vec::new(),
    }
}

fn numbered_mcp_tools(count: usize) -> Vec<ToolInfo> {
    (0..count)
        .map(|index| {
            let tool_name = format!("tool_{index}");
            make_mcp_tool(
                "rmcp",
                &tool_name,
                "mcp__rmcp",
                &tool_name,
                /*connector_id*/ None,
                /*connector_name*/ None,
            )
        })
        .collect()
}

fn expected_runtimes(
    tools: &[ToolInfo],
    exposure: ToolExposure,
) -> HashMap<ToolName, ToolExposure> {
    tools
        .iter()
        .map(|tool| (tool.canonical_tool_name(), exposure))
        .collect()
}

fn runtimes_by_name(
    tools: &[ToolInfo],
    config: &Config,
    apps_enabled: bool,
    search_tool_enabled: bool,
) -> HashMap<ToolName, ToolExposure> {
    runtimes_by_name_with_catalog(
        tools,
        config,
        apps_enabled,
        &ResolvedMcpCatalog::default(),
        search_tool_enabled,
    )
}

fn runtimes_by_name_with_catalog(
    tools: &[ToolInfo],
    config: &Config,
    apps_enabled: bool,
    mcp_server_catalog: &ResolvedMcpCatalog,
    search_tool_enabled: bool,
) -> HashMap<ToolName, ToolExposure> {
    let mut handlers = HashMap::new();
    let mut registry = ToolRegistry::default();
    append_mcp_tools(
        tools,
        config,
        apps_enabled,
        mcp_server_catalog,
        search_tool_enabled,
        &mut handlers,
        &mut registry,
    );
    registry
        .entries()
        .map(|tool| (tool.runtime.tool_name(), tool.exposure))
        .collect()
}

fn append_tool_and_capture_runtime(
    tool: &ToolInfo,
    config: &Config,
    catalog: &ResolvedMcpCatalog,
    handlers: &mut HashMap<ToolName, CachedMcpHandler>,
) -> Arc<dyn CoreToolRuntime> {
    let mut registry = ToolRegistry::default();
    let registered = append_mcp_tools(
        std::slice::from_ref(tool),
        config,
        /*apps_enabled*/ false,
        catalog,
        /*search_tool_enabled*/ false,
        handlers,
        &mut registry,
    );
    assert_eq!(registered, HashSet::from([tool.canonical_tool_name()]));
    Arc::clone(
        &registry
            .entries()
            .next()
            .expect("the MCP tool should be registered")
            .runtime,
    )
}

#[tokio::test]
async fn reuses_mcp_handler_for_equivalent_tool_metadata_across_appends() {
    let config = test_config().await;
    let catalog = ResolvedMcpCatalog::default();
    let tool = make_mcp_tool(
        "rmcp",
        "read",
        "mcp__rmcp",
        "read",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let mut handlers = HashMap::new();

    let first = append_tool_and_capture_runtime(&tool, &config, &catalog, &mut handlers);
    let second = append_tool_and_capture_runtime(&tool, &config, &catalog, &mut handlers);

    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn replaces_mcp_handler_when_any_tool_metadata_changes() {
    let config = test_config().await;
    let catalog = ResolvedMcpCatalog::default();
    let mutations: [(&str, fn(&mut ToolInfo)); 18] = [
        ("server name", |tool| {
            tool.server_name = "other_server".to_owned()
        }),
        ("parallel support", |tool| {
            tool.supports_parallel_tool_calls = true
        }),
        ("server origin", |tool| {
            tool.server_origin = Some("https://example.test/mcp".to_owned())
        }),
        ("callable name", |tool| {
            tool.callable_name = "renamed".to_owned()
        }),
        ("callable namespace", |tool| {
            tool.callable_namespace = "mcp__renamed".to_owned()
        }),
        ("namespace description", |tool| {
            tool.namespace_description = Some("updated namespace".to_owned())
        }),
        ("raw tool name", |tool| tool.tool.name = "renamed".into()),
        ("tool title", |tool| {
            tool.tool.title = Some("Updated title".to_owned())
        }),
        ("tool description", |tool| {
            tool.tool.description = Some("Updated description".into())
        }),
        ("input schema", |tool| {
            tool.tool.input_schema = Arc::new(
                serde_json::json!({
                    "type": "object",
                    "properties": { "updated": { "type": "string" } }
                })
                .as_object()
                .expect("input schema object")
                .clone(),
            )
        }),
        ("output schema", |tool| {
            tool.tool.output_schema = Some(Arc::new(
                serde_json::json!({ "type": "string" })
                    .as_object()
                    .expect("output schema object")
                    .clone(),
            ))
        }),
        ("tool annotations", |tool| {
            tool.tool.annotations = Some(ToolAnnotations::new().read_only(true))
        }),
        ("tool icons", |tool| {
            tool.tool.icons = Some(vec![Icon::new("https://example.test/icon.png")])
        }),
        ("tool metadata", |tool| {
            tool.tool.meta = Some(MetaObject(
                serde_json::json!({ "updated": true })
                    .as_object()
                    .expect("tool metadata object")
                    .clone(),
            ))
        }),
        ("optional file fields", |tool| {
            tool.openai_file_input_optional_fields
                .insert("file".to_owned(), vec!["optional".to_owned()]);
        }),
        ("connector id", |tool| {
            tool.connector_id = Some("connector".to_owned())
        }),
        ("connector name", |tool| {
            tool.connector_name = Some("Connector".to_owned())
        }),
        ("plugin display names", |tool| {
            tool.plugin_display_names = vec!["Plugin".to_owned()]
        }),
    ];

    for (field, mutate) in mutations {
        let mut handlers = HashMap::new();
        let original = make_mcp_tool(
            "rmcp",
            "read",
            "mcp__rmcp",
            "read",
            /*connector_id*/ None,
            /*connector_name*/ None,
        );
        let first = append_tool_and_capture_runtime(&original, &config, &catalog, &mut handlers);
        let mut updated = original;
        mutate(&mut updated);
        let replacement =
            append_tool_and_capture_runtime(&updated, &config, &catalog, &mut handlers);

        assert!(
            !Arc::ptr_eq(&first, &replacement),
            "changing {field} must replace the cached handler"
        );
    }
}

#[tokio::test]
async fn agent_plugin_status_rebuilds_handler_and_preserves_original_metadata() {
    let codex_home = tempdir().expect("create config directory");
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        "[mcp_servers.agent]\ncommand = \"echo\"\n",
    )
    .expect("write config");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config should build");
    let mut agent_catalog = ResolvedMcpCatalog::builder();
    agent_catalog.register(McpServerRegistration::from_plugin(
        "agent".to_owned(),
        McpPluginAttribution::agent_plugin("agent@test".to_owned(), "Agent".to_owned()),
        /*plugin_order*/ 0,
        config.mcp_servers.get()["agent"].clone(),
    ));
    let agent_catalog = agent_catalog.build();
    let regular_catalog = ResolvedMcpCatalog::default();
    let long_description = "n".repeat(1_500);
    let mut tool = make_mcp_tool(
        "agent",
        "read",
        "mcp__agent",
        "read",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    tool.namespace_description = Some(long_description.clone());
    let name = tool.canonical_tool_name();
    let mut handlers = HashMap::new();

    let regular = append_tool_and_capture_runtime(&tool, &config, &regular_catalog, &mut handlers);
    let agent = append_tool_and_capture_runtime(&tool, &config, &agent_catalog, &mut handlers);
    assert!(!Arc::ptr_eq(&regular, &agent));
    assert_eq!(
        agent
            .search_info()
            .expect("agent handler search info")
            .source_info
            .expect("agent source info")
            .description
            .expect("agent namespace description")
            .len(),
        1_000,
        "the agent handler should retain its namespace-description truncation"
    );
    assert_eq!(
        handlers[&name].tool_info.namespace_description.as_deref(),
        Some(long_description.as_str()),
        "cache metadata must remain the original untruncated catalog value"
    );

    let agent_again =
        append_tool_and_capture_runtime(&tool, &config, &agent_catalog, &mut handlers);
    assert!(Arc::ptr_eq(&agent, &agent_again));
    let regular_again =
        append_tool_and_capture_runtime(&tool, &config, &regular_catalog, &mut handlers);
    assert!(!Arc::ptr_eq(&agent_again, &regular_again));
}

#[tokio::test]
async fn evicts_removed_mcp_tools_including_the_last_cached_handler() {
    let config = test_config().await;
    let catalog = ResolvedMcpCatalog::default();
    let tools = numbered_mcp_tools(/*count*/ 2);
    let mut handlers = HashMap::new();
    let mut registry = ToolRegistry::default();
    append_mcp_tools(
        &tools,
        &config,
        /*apps_enabled*/ false,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut registry,
    );
    assert_eq!(handlers.len(), 2);

    let remaining_tool = tools[1].clone();
    let mut registry = ToolRegistry::default();
    let registered = append_mcp_tools(
        std::slice::from_ref(&remaining_tool),
        &config,
        /*apps_enabled*/ false,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut registry,
    );
    assert_eq!(
        registered,
        HashSet::from([remaining_tool.canonical_tool_name()])
    );
    assert_eq!(handlers.len(), 1);

    let mut registry = ToolRegistry::default();
    let registered = append_mcp_tools(
        &[],
        &config,
        /*apps_enabled*/ false,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut registry,
    );
    assert!(registered.is_empty());
    assert!(handlers.is_empty());
}

#[tokio::test]
async fn failed_metadata_rebuild_evicts_the_previous_handler() {
    let config = test_config().await;
    let catalog = ResolvedMcpCatalog::default();
    let valid = make_mcp_tool(
        "rmcp",
        "read",
        "mcp__rmcp",
        "read",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let name = valid.canonical_tool_name();
    let mut handlers = HashMap::new();
    let first = append_tool_and_capture_runtime(&valid, &config, &catalog, &mut handlers);

    let mut invalid = valid.clone();
    invalid.tool.input_schema = Arc::new(
        serde_json::json!({ "type": "null" })
            .as_object()
            .expect("invalid schema remains a JSON object")
            .clone(),
    );
    assert!(
        McpHandler::new(invalid.clone()).is_err(),
        "singleton-null input schemas must be a reproducible handler-build failure"
    );

    let mut registry = ToolRegistry::default();
    let registered = append_mcp_tools(
        std::slice::from_ref(&invalid),
        &config,
        /*apps_enabled*/ false,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut registry,
    );
    assert!(registered.is_empty());
    assert!(registry.entries().next().is_none());
    assert!(!handlers.contains_key(&name));

    let rebuilt = append_tool_and_capture_runtime(&valid, &config, &catalog, &mut handlers);
    assert!(!Arc::ptr_eq(&first, &rebuilt));
}

#[tokio::test]
async fn agent_plugin_budget_hides_only_overflow_agent_tools() {
    let codex_home = tempdir().expect("tempdir should succeed");
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        "[mcp_servers.agent]\ncommand = \"echo\"\n",
    )
    .expect("write config");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config should build");
    let agent_config = config.mcp_servers.get()["agent"].clone();
    let legacy_config = agent_config.clone();
    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_plugin(
        "agent".to_string(),
        McpPluginAttribution::agent_plugin("agent@test".to_string(), "Agent".to_string()),
        /*plugin_order*/ 0,
        agent_config,
    ));
    catalog.register(McpServerRegistration::from_plugin(
        "legacy".to_string(),
        McpPluginAttribution::new("legacy@test".to_string(), "Legacy".to_string()),
        /*plugin_order*/ 1,
        legacy_config,
    ));
    let catalog = catalog.build();
    let mut tools = (0..40)
        .map(|index| {
            let name = format!("tool_{index}");
            let mut tool = make_mcp_tool(
                "agent",
                &name,
                "mcp__agent",
                &name,
                /*connector_id*/ None,
                /*connector_name*/ None,
            );
            tool.namespace_description = Some("n".repeat(1_000));
            tool.tool.description = Some("d".repeat(1_000).into());
            tool
        })
        .collect::<Vec<_>>();
    let oversized_name = "x".repeat(MAX_AGENT_PLUGIN_MCP_SPEC_BYTES);
    let oversized_agent_tool = make_mcp_tool(
        "agent",
        "oversized_agent_tool",
        "mcp__agent",
        &oversized_name,
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    tools.push(oversized_agent_tool.clone());
    let legacy_tool = make_mcp_tool(
        "legacy",
        "legacy_tool",
        "mcp__legacy",
        &oversized_name,
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    tools.push(legacy_tool.clone());

    let runtimes = runtimes_by_name_with_catalog(
        &tools, &config, /*apps_enabled*/ false, &catalog, /*search_tool_enabled*/ false,
    );
    let agent_exposures = tools[..40]
        .iter()
        .map(|tool| runtimes[&tool.canonical_tool_name()])
        .collect::<Vec<_>>();

    assert!(agent_exposures.contains(&ToolExposure::Direct));
    assert!(agent_exposures.contains(&ToolExposure::Hidden));
    assert_eq!(
        runtimes[&oversized_agent_tool.canonical_tool_name()],
        ToolExposure::Hidden
    );
    assert_eq!(
        runtimes[&legacy_tool.canonical_tool_name()],
        ToolExposure::Direct
    );
}

fn with_visibility(mut tool: ToolInfo, visibility: &[&str]) -> ToolInfo {
    tool.tool.meta = Some(MetaObject(
        serde_json::json!({ "ui": { "visibility": visibility } })
            .as_object()
            .expect("metadata object")
            .clone(),
    ));
    tool
}

#[tokio::test]
async fn directly_exposes_effective_tool_sets_when_search_is_unavailable() {
    let config = test_config().await;
    let mcp_tools = numbered_mcp_tools(/*count*/ 2);

    let runtimes = runtimes_by_name(
        &mcp_tools, &config, /*apps_enabled*/ false, /*search_tool_enabled*/ false,
    );

    assert_eq!(
        runtimes,
        expected_runtimes(&mcp_tools, ToolExposure::Direct)
    );
}

#[tokio::test]
async fn cached_app_handlers_still_obey_current_apps_enablement_and_tool_policy() {
    let config = test_config().await;
    let codex_home = tempdir().expect("create restrictive config directory");
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        "[apps.calendar]\ndefault_tools_enabled = false\n",
    )
    .expect("write restrictive app policy");
    let restricted_config = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("build restrictive app policy");
    let tools = [make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "events/create",
        "mcp__codex_apps__calendar",
        "create",
        Some("calendar"),
        Some("Calendar"),
    )];
    let mut handlers = HashMap::new();
    let catalog = ResolvedMcpCatalog::default();
    let mut allowed_registry = ToolRegistry::default();
    let allowed = append_mcp_tools(
        &tools,
        &config,
        /*apps_enabled*/ true,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut allowed_registry,
    );
    let cached_handler = &allowed_registry
        .entries()
        .next()
        .expect("allowed app tool should be registered")
        .runtime;

    let mut disabled_registry = ToolRegistry::default();
    let disabled = append_mcp_tools(
        &tools,
        &config,
        /*apps_enabled*/ false,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut disabled_registry,
    );
    let mut restricted_registry = ToolRegistry::default();
    let restricted = append_mcp_tools(
        &tools,
        &restricted_config,
        /*apps_enabled*/ true,
        &catalog,
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut restricted_registry,
    );
    let mut restored_registry = ToolRegistry::default();
    let restored = append_mcp_tools(
        &tools,
        &config,
        /*apps_enabled*/ true,
        &catalog,
        /*search_tool_enabled*/ true,
        &mut handlers,
        &mut restored_registry,
    );
    let restored_handler = restored_registry
        .entries()
        .next()
        .expect("restored app tool should be registered");

    assert_eq!(allowed, HashSet::from([tools[0].canonical_tool_name()]));
    assert!(disabled.is_empty());
    assert!(restricted.is_empty());
    assert_eq!(restored, allowed);
    assert!(Arc::ptr_eq(cached_handler, &restored_handler.runtime));
    assert_eq!(restored_handler.exposure, ToolExposure::Deferred);
}

#[tokio::test]
async fn excludes_tools_hidden_from_model_exposure() {
    let config = test_config().await;
    let visible_tool = make_mcp_tool(
        "rmcp",
        "visible_tool",
        "mcp__rmcp",
        "visible_tool",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let hidden_tool = with_visibility(
        make_mcp_tool(
            "rmcp",
            "hidden_tool",
            "mcp__rmcp",
            "hidden_tool",
            /*connector_id*/ None,
            /*connector_name*/ None,
        ),
        &["app"],
    );
    let empty_visibility_tool = with_visibility(
        make_mcp_tool(
            "rmcp",
            "empty_visibility_tool",
            "mcp__rmcp",
            "empty_visibility_tool",
            /*connector_id*/ None,
            /*connector_name*/ None,
        ),
        &[],
    );
    let visible_app_tool = with_visibility(
        make_mcp_tool(
            CODEX_APPS_MCP_SERVER_NAME,
            "calendar_read",
            "mcp__codex_apps__calendar",
            "read",
            Some("calendar"),
            Some("Calendar"),
        ),
        &["app", "model"],
    );
    let hidden_app_tool = with_visibility(
        make_mcp_tool(
            CODEX_APPS_MCP_SERVER_NAME,
            "calendar_open",
            "mcp__codex_apps__calendar",
            "open",
            Some("calendar"),
            Some("Calendar"),
        ),
        &["app"],
    );
    let mcp_tools = vec![
        visible_tool.clone(),
        hidden_tool,
        empty_visibility_tool,
        visible_app_tool.clone(),
        hidden_app_tool,
    ];
    let runtimes = runtimes_by_name(
        &mcp_tools, &config, /*apps_enabled*/ true, /*search_tool_enabled*/ false,
    );

    assert_eq!(
        runtimes,
        expected_runtimes(&[visible_tool, visible_app_tool], ToolExposure::Direct)
    );
}

#[tokio::test]
async fn app_tool_registration_uses_trusted_catalog_metadata_and_preserves_source_order() {
    let config = test_config().await;
    let app_tool = make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "calendar_list_events",
        "mcp__codex_apps__calendar",
        "list_events",
        Some("calendar"),
        Some("Calendar"),
    );
    let missing_connector_id = make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "unknown_tool",
        "mcp__codex_apps__unknown",
        "unknown",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let mut synthetic_app_tool = make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "gmail_batch_read_email",
        "mcp__codex_apps__gmail",
        "batch_read_email",
        Some("gmail"),
        Some("Gmail"),
    );
    synthetic_app_tool.tool.meta = Some(MetaObject(
        serde_json::json!({ "_codex_apps": { "synthetic_link": true } })
            .as_object()
            .expect("metadata should be an object")
            .clone(),
    ));
    let regular_tool = make_mcp_tool(
        "rmcp",
        "regular_tool",
        "mcp__rmcp",
        "regular_tool",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let mcp_tools = [
        app_tool.clone(),
        missing_connector_id,
        synthetic_app_tool.clone(),
        regular_tool.clone(),
    ];
    let mut handlers = HashMap::new();
    let mut registry = ToolRegistry::default();

    append_mcp_tools(
        &mcp_tools,
        &config,
        /*apps_enabled*/ true,
        &ResolvedMcpCatalog::default(),
        /*search_tool_enabled*/ false,
        &mut handlers,
        &mut registry,
    );

    let registered_names = registry
        .entries()
        .map(|entry| entry.runtime.tool_name())
        .collect::<Vec<_>>();
    assert_eq!(
        registered_names,
        vec![
            regular_tool.canonical_tool_name(),
            app_tool.canonical_tool_name(),
            synthetic_app_tool.canonical_tool_name(),
        ]
    );
    assert_eq!(
        runtimes_by_name(
            &mcp_tools, &config, /*apps_enabled*/ false, /*search_tool_enabled*/ false,
        ),
        expected_runtimes(&[regular_tool], ToolExposure::Direct)
    );
}

#[tokio::test]
async fn applies_per_tool_app_policy_across_the_exposure_build() {
    let codex_home = tempdir().expect("tempdir should succeed");
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        r#"
[apps.calendar]
default_tools_enabled = false

[apps.calendar.tools."events/create"]
enabled = true
"#,
    )
    .expect("write config");
    let config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("config should build");
    let enabled_tool = make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "events/create",
        "mcp__codex_apps__calendar",
        "create",
        Some("calendar"),
        Some("Calendar"),
    );
    let disabled_tool = make_mcp_tool(
        CODEX_APPS_MCP_SERVER_NAME,
        "events/list",
        "mcp__codex_apps__calendar",
        "list",
        Some("calendar"),
        Some("Calendar"),
    );
    let mcp_tools = [enabled_tool.clone(), disabled_tool];
    let runtimes = runtimes_by_name(
        &mcp_tools, &config, /*apps_enabled*/ true, /*search_tool_enabled*/ false,
    );

    assert_eq!(
        runtimes,
        expected_runtimes(&[enabled_tool], ToolExposure::Direct)
    );
}

#[tokio::test]
async fn defers_effective_tool_sets_when_search_is_available() {
    let config = test_config().await;
    let mcp_tools = numbered_mcp_tools(/*count*/ 2);

    let runtimes = runtimes_by_name(
        &mcp_tools, &config, /*apps_enabled*/ false, /*search_tool_enabled*/ true,
    );

    assert_eq!(
        runtimes,
        expected_runtimes(&mcp_tools, ToolExposure::Deferred)
    );
}

#[tokio::test]
async fn defers_apps_and_non_app_mcp_tools() {
    let config = test_config().await;
    let mcp_tools = vec![
        make_mcp_tool(
            "rmcp",
            "tool",
            "mcp__rmcp",
            "tool",
            /*connector_id*/ None,
            /*connector_name*/ None,
        ),
        make_mcp_tool(
            CODEX_APPS_MCP_SERVER_NAME,
            "calendar_create_event",
            "mcp__codex_apps__calendar",
            "_create_event",
            Some("calendar"),
            Some("Calendar"),
        ),
    ];
    let runtimes = runtimes_by_name(
        &mcp_tools, &config, /*apps_enabled*/ true, /*search_tool_enabled*/ true,
    );

    assert_eq!(
        runtimes,
        expected_runtimes(&mcp_tools, ToolExposure::Deferred)
    );
}
