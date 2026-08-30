use anyhow::Result;
use codex_core::config::Config;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolContributor;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolCall;
use codex_tools::ToolExecutor;
use codex_tools::ToolExecutorFuture;
use codex_tools::ToolName;
use codex_tools::ToolOutput;
use codex_tools::ToolPayload;
use codex_tools::ToolResult;
use codex_tools::ToolSpec;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
const TOOL_NAME: &str = "egress_test_tool";
const CALL_ID: &str = "egress-test-call";
struct EgressTestTool(Option<ToolResult>);
impl ToolContributor for EgressTestTool {
    fn tools(&self, _: &ExtensionData, _: &ExtensionData) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        vec![Arc::new(Self(self.0.clone()))]
    }
}
impl ToolExecutor<ToolCall> for EgressTestTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: TOOL_NAME.to_string(),
            description: "Returns a deliberately divergent tool result.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
            output_schema: None,
        })
    }

    fn handle(&self, _call: ToolCall) -> ToolExecutorFuture<'_> {
        let output = EgressTestOutput(self.0.clone());
        Box::pin(async move { Ok(Box::new(output) as Box<dyn ToolOutput>) })
    }
}
struct EgressTestOutput(Option<ToolResult>);
impl ToolOutput for EgressTestOutput {
    fn log_output(&self) -> String {
        "legacy".to_string()
    }
    fn success_for_logging(&self) -> bool {
        true
    }
    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_text("legacy".to_string()),
        }
    }
    fn to_tool_result(&self) -> Option<ToolResult> {
        self.0.clone()
    }
}

async fn assert_egress(canonical: Option<ToolResult>, expected: &str) -> Result<()> {
    let server = responses::start_mock_server().await;
    let mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("resp-1"),
                responses::ev_function_call(CALL_ID, TOOL_NAME, "{}"),
                responses::ev_completed("resp-1"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("msg-1", "done"),
                responses::ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.tool_contributor(Arc::new(EgressTestTool(canonical)));
    let test = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("Call the egress test tool.").await?;
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    let output = requests[1].function_call_output(CALL_ID)["output"].clone();
    assert_eq!(output, json!(expected));
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_loop_uses_tool_result_egress() -> Result<()> {
    skip_if_no_network!(Ok(()));
    for (canonical, expected) in [
        (Some(ToolResult::success_text("canonical")), "canonical"),
        (None, "legacy"),
    ] {
        assert_egress(canonical, expected).await?;
    }
    Ok(())
}
