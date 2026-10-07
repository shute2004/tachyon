use anyhow::Result;
use codex_core::StartThreadOptions;
use codex_features::Feature;
use codex_protocol::dynamic_tools::DynamicToolFunctionSpec;
use codex_protocol::dynamic_tools::DynamicToolNamespaceSpec;
use codex_protocol::dynamic_tools::DynamicToolNamespaceTool;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use core_test_support::apps_test_server::configure_search_capable_model;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descriptions_share_space_without_hiding_late_namespaces() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let description = "Find records & retrieve <details> from this service. Search by project, owner, date, or status, then read the matching record to answer questions with its title, summary, source link, and relevant activity from the connected workspace.";
    let namespaces = (0..19)
        .map(|index| format!("service_{index:02}"))
        .chain(std::iter::once("zulu_travel".to_string()))
        .collect::<Vec<_>>();
    let input_schema = json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    });
    let dynamic_tools = namespaces
        .iter()
        .map(|name| {
            DynamicToolSpec::Namespace(DynamicToolNamespaceSpec {
                name: name.clone(),
                description: description.to_string(),
                tools: vec![DynamicToolNamespaceTool::Function(
                    DynamicToolFunctionSpec {
                        name: "search_records".to_string(),
                        description: "Search records in this service.".to_string(),
                        input_schema: input_schema.clone(),
                        defer_loading: true,
                    },
                )],
            })
        })
        .collect();

    let mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_tool_search_call(
                    "find-travel",
                    &json!({"query": "zulu_travel", "limit": 1}),
                ),
                responses::ev_completed("search"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message(
                    "found",
                    "Zulu Travel provides a record search tool.",
                ),
                responses::ev_completed("found"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("follow-up", "The same services are available."),
                responses::ev_completed("follow-up"),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex().with_config(|config| {
        configure_search_capable_model(config);
        config.agents_enabled = false;
        config
            .features
            .enable(Feature::DeferredToolWorldState)
            .expect("test config should allow feature update");
    });
    let base_test = builder.build_with_auto_env(&server).await?;
    let environment = base_test.executor_environment().selection().clone();
    let new_thread = base_test
        .thread_manager
        .start_thread(StartThreadOptions {
            dynamic_tools,
            environments: Some(vec![environment]),
            ..StartThreadOptions::new(base_test.config.clone())
        })
        .await?;
    let mut test = base_test;
    test.codex = new_thread.thread;
    test.session_configured = new_thread.session_configured;

    test.submit_turn("Find the record search tool for Zulu Travel.")
        .await?;
    test.submit_turn("Are the same services still available?")
        .await?;

    let requests = mock.requests();
    assert_eq!(requests.len(), 3);
    let tools_sections = requests
        .iter()
        .map(|request| {
            request
                .message_input_texts("developer")
                .into_iter()
                .filter(|text| text.starts_with("<tools>"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let [fragment] = tools_sections[0].as_slice() else {
        panic!("expected one initial tools fragment");
    };
    assert!(fragment.len() <= 4096);
    let advertised = fragment
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| {
            line.split_once(": ")
                .expect("namespace line has a description")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        advertised.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        namespaces.iter().map(String::as_str).collect::<Vec<_>>(),
    );

    let escaped_description = description
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    assert!(advertised.iter().all(|(_, shortened)| {
        shortened.strip_suffix("...").is_some_and(|prefix| {
            !prefix.is_empty()
                && prefix.len() < escaped_description.len()
                && escaped_description.starts_with(prefix)
        })
    }));
    assert!(fragment.contains("Find records &amp; retrieve &lt;details&gt;"));
    assert_eq!(tools_sections, vec![vec![fragment.clone()]; 3]);
    assert_eq!(
        requests[1].tool_search_output("find-travel")["tools"],
        json!([{
            "type": "namespace",
            "name": "zulu_travel",
            "description": description,
            "tools": [{
                "type": "function",
                "name": "search_records",
                "description": "Search records in this service.",
                "strict": false,
                "defer_loading": true,
                "parameters": input_schema,
            }],
        }]),
    );

    Ok(())
}
