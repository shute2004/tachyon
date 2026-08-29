use super::ToolCallSource;
use super::ToolRouter;
use crate::model_runtime::ir::ModelToolCall;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolInput;
use crate::tools::context::ToolPayload;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolName;
use serde_json::json;

#[test]
fn canonical_json_invocation_builds_function_payload() {
    let call = ToolRouter::build_model_invocation_call(ModelToolCall {
        call_id: ModelToolCallId("call-1".to_string()),
        namespace: Some("workspace".to_string()),
        name: "read_file".to_string(),
        input: ModelToolInput::Json(json!({"path": "README.md"})),
    });

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("workspace", "read_file")
    );
    assert_eq!(call.call_id, "call-1");
    assert_eq!(call.encrypted_function_args, None);
    assert_eq!(call.direct_source(), ToolCallSource::Direct);
    let ToolPayload::Function { arguments } = call.payload else {
        panic!("expected function payload");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&arguments).expect("canonical JSON arguments"),
        json!({"path": "README.md"})
    );
}

#[test]
fn canonical_text_invocation_builds_custom_payload() {
    let call = ToolRouter::build_model_invocation_call(ModelToolCall {
        call_id: ModelToolCallId("call-2".to_string()),
        namespace: Some("mcp__python".to_string()),
        name: "exec".to_string(),
        input: ModelToolInput::Text("print('hello')".to_string()),
    });

    assert_eq!(call.tool_name, ToolName::namespaced("mcp__python", "exec"));
    assert_eq!(call.call_id, "call-2");
    assert_eq!(call.encrypted_function_args, None);
    assert_eq!(call.direct_source(), ToolCallSource::Direct);
    assert_eq!(
        call.payload,
        ToolPayload::Custom {
            input: "print('hello')".to_string()
        }
    );
}

#[test]
fn codex_function_decoration_preserves_plaintext_collaboration_source_and_raw_json() {
    let original_arguments = "{ \"target\" : \"agent-1\", \"message\" : \"hello\" }";
    let call = ToolRouter::build_tool_call(ResponseItem::FunctionCall {
        id: None,
        name: "send_message".to_string(),
        namespace: Some("collaboration".to_string()),
        arguments: original_arguments.to_string(),
        encrypted_function_args: Some(Vec::new()),
        call_id: "call-collaboration".to_string(),
        internal_chat_message_metadata_passthrough: None,
    })
    .expect("representable function call should convert")
    .expect("function call should enter tool runtime");

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("collaboration", "send_message")
    );
    assert_eq!(call.direct_source(), ToolCallSource::DirectPlaintextMessage);
    assert_eq!(call.encrypted_function_args, Some(Vec::new()));
    assert_eq!(
        call.payload,
        ToolPayload::Function {
            arguments: original_arguments.to_string()
        }
    );
}

#[test]
fn client_tool_search_uses_canonical_json_semantics_before_discovery_payload() {
    let call = ToolRouter::build_tool_call(ResponseItem::ToolSearchCall {
        id: None,
        call_id: Some("search-1".to_string()),
        status: Some("completed".to_string()),
        execution: "client".to_string(),
        arguments: json!({"query": "calendar", "limit": 2}),
        internal_chat_message_metadata_passthrough: None,
    })
    .expect("client discovery call should convert")
    .expect("client discovery call should enter tool runtime");

    assert_eq!(call.tool_name, ToolName::plain("tool_search"));
    assert_eq!(call.call_id, "search-1");
    assert!(matches!(call.payload, ToolPayload::ToolSearch { .. }));
    assert_eq!(call.encrypted_function_args, None);
}

#[test]
fn provider_owned_tool_search_stays_outside_canonical_tool_runtime_ingress() {
    let call = ToolRouter::build_tool_call(ResponseItem::ToolSearchCall {
        id: None,
        call_id: Some("search-server".to_string()),
        status: Some("completed".to_string()),
        execution: "server".to_string(),
        arguments: json!({"query": "calendar"}),
        internal_chat_message_metadata_passthrough: None,
    })
    .expect("provider-owned discovery should not be a local tool error");

    assert_eq!(call, None);
}
