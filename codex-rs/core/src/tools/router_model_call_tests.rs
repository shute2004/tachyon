use super::ToolCallSource;
use super::ToolRouter;
use crate::model_runtime::ir::ModelToolCall;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolInput;
use crate::tools::context::ToolPayload;
use codex_tools::ToolName;
use serde_json::json;

#[test]
fn canonical_json_invocation_builds_function_call_and_preserves_codex_decoration() {
    let call = ToolRouter::build_model_invocation_call(
        ModelToolCall {
            call_id: ModelToolCallId("call-1".to_string()),
            namespace: Some("collaboration".to_string()),
            name: "send_message".to_string(),
            input: ModelToolInput::Json(json!({
                "target": "agent-1",
                "message": "hello"
            })),
        },
        Some(Vec::new()),
    );

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("collaboration", "send_message")
    );
    assert_eq!(call.call_id, "call-1");
    assert_eq!(call.encrypted_function_args, Some(Vec::new()));
    assert_eq!(call.direct_source(), ToolCallSource::DirectPlaintextMessage);
    let ToolPayload::Function { arguments } = call.payload else {
        panic!("expected function payload");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&arguments).expect("canonical JSON arguments"),
        json!({"target": "agent-1", "message": "hello"})
    );
}

#[test]
fn canonical_text_invocation_builds_custom_call_without_codex_function_decoration() {
    let call = ToolRouter::build_model_invocation_call(
        ModelToolCall {
            call_id: ModelToolCallId("call-2".to_string()),
            namespace: Some("mcp__python".to_string()),
            name: "exec".to_string(),
            input: ModelToolInput::Text("print('hello')".to_string()),
        },
        Some(Vec::new()),
    );

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("mcp__python", "exec")
    );
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
