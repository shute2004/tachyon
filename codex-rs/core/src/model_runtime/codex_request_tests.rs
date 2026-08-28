use super::*;

use codex_protocol::ResponseItemId;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_tools::JsonSchema;
use pretty_assertions::assert_eq;
use serde_json::json;

fn prompt_with(input: Vec<ResponseItem>, tools: Vec<ToolSpec>) -> Prompt {
    Prompt {
        input,
        tools: Arc::from(tools),
        parallel_tool_calls: true,
        base_instructions: BaseInstructions {
            text: "base instructions".to_string(),
            provenance: None,
        },
        output_schema: Some(json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        })),
        output_schema_strict: true,
        cyber_access_program: None,
    }
}

fn assert_prompt_request_semantics_round_trip(prompt: &Prompt) -> ModelRequest {
    let request = try_model_request_from_prompt(prompt).expect("prompt should be canonicalizable");
    let rebuilt = prompt_from_model_request(&request, prompt).expect("request should rebuild");

    assert_eq!(rebuilt.input, prompt.input);
    assert_eq!(rebuilt.tools.as_ref(), prompt.tools.as_ref());
    assert_eq!(rebuilt.parallel_tool_calls, prompt.parallel_tool_calls);
    assert_eq!(rebuilt.base_instructions, prompt.base_instructions);
    assert_eq!(rebuilt.output_schema, prompt.output_schema);
    assert_eq!(rebuilt.output_schema_strict, prompt.output_schema_strict);
    assert_eq!(rebuilt.cyber_access_program, prompt.cyber_access_program);

    request
}

#[test]
fn message_and_output_contract_round_trip_preserves_codex_decorations() {
    let item_id = ResponseItemId::with_suffix("msg", "request-bridge");
    let metadata = InternalChatMessageMetadataPassthrough {
        turn_id: Some("turn-1".to_string()),
        ..Default::default()
    };
    let prompt = prompt_with(
        vec![ResponseItem::Message {
            id: Some(item_id),
            role: "user".to_string(),
            content: vec![
                ContentItem::InputText {
                    text: "inspect this".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,abc".to_string(),
                    detail: Some(ImageDetail::High),
                },
            ],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(metadata),
        }],
        Vec::new(),
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert_eq!(request.instructions, "base instructions");
    assert!(matches!(
        request.output.format,
        ModelOutputFormat::JsonSchema { strict: true, .. }
    ));
    assert!(matches!(
        request.input.as_slice(),
        [ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            ..
        })]
    ));
}

#[test]
fn reasoning_round_trip_preserves_provider_private_continuation() {
    let metadata = InternalChatMessageMetadataPassthrough {
        turn_id: Some("turn-reasoning".to_string()),
        ..Default::default()
    };
    let reasoning = ResponseItem::Reasoning {
        id: Some(ResponseItemId::with_suffix("rs", "request-bridge")),
        summary: vec![
            ReasoningItemReasoningSummary::SummaryText {
                text: "first summary".to_string(),
            },
            ReasoningItemReasoningSummary::SummaryText {
                text: "second summary".to_string(),
            },
        ],
        content: Some(vec![
            ReasoningItemContent::ReasoningText {
                text: "internal reasoning".to_string(),
            },
            ReasoningItemContent::Text {
                text: "exposed reasoning".to_string(),
            },
        ]),
        encrypted_content: Some("opaque-provider-continuation".to_string()),
        internal_chat_message_metadata_passthrough: Some(metadata),
    };
    let prompt = prompt_with(vec![reasoning.clone()], Vec::new());

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert_eq!(
        request.input,
        vec![ModelInputItem::Reasoning(ModelReasoning {
            summary: vec!["first summary".to_string(), "second summary".to_string(),],
            content: vec![
                "internal reasoning".to_string(),
                "exposed reasoning".to_string(),
            ],
        })]
    );

    let rebuilt = prompt_from_model_request(&request, &prompt).expect("round trip");
    assert_eq!(rebuilt.input, vec![reasoning]);
}

#[test]
fn grammar_and_deferred_freeform_tool_round_trip() {
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description: "Apply a patch".to_string(),
            defer_loading: Some(true),
            format: FreeformToolFormat {
                r#type: FREEFORM_GRAMMAR_FORMAT.to_string(),
                syntax: "lark".to_string(),
                definition: "start: patch".to_string(),
            },
        })],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert!(matches!(
        request.tools.as_slice(),
        [ModelToolSpec::Freeform {
            input_format: ModelFreeformInputFormat::Grammar { .. },
            availability: ModelToolAvailability::Deferred,
            purpose: ModelToolPurpose::Invocation,
            ..
        }]
    ));
}

#[test]
fn client_tool_search_maps_to_discovery_semantics_without_wire_variant() {
    let parameters: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {"query": {"type": "string"}},
        "required": ["query"],
        "additionalProperties": false
    }))
    .expect("valid schema");
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::ToolSearch {
            execution: TOOL_SEARCH_CLIENT_EXECUTION.to_string(),
            description: "Find additional tools".to_string(),
            parameters,
        }],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert!(matches!(
        request.tools.as_slice(),
        [ModelToolSpec::Function {
            name,
            purpose: ModelToolPurpose::Discovery,
            availability: ModelToolAvailability::Immediate,
            ..
        }] if name == TOOL_SEARCH_NAME
    ));
}

#[test]
fn default_namespace_description_round_trips_as_namespace_semantics() {
    let parameters: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    }))
    .expect("valid schema");
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Namespace(ResponsesApiNamespace {
            name: "workspace".to_string(),
            description: default_namespace_description("workspace"),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                strict: false,
                defer_loading: None,
                parameters,
                output_schema: None,
            })],
        })],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert!(matches!(
        request.tools.as_slice(),
        [ModelToolSpec::Function {
            namespace: Some(namespace),
            name,
            ..
        }] if namespace == "workspace" && name == "read_file"
    ));
}

#[test]
fn custom_namespace_description_stays_on_legacy_path() {
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Namespace(ResponsesApiNamespace {
            name: "workspace".to_string(),
            description: "Custom namespace guidance that the canonical IR cannot preserve"
                .to_string(),
            tools: Vec::new(),
        })],
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn provider_web_search_tool_stays_on_legacy_path() {
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::WebSearch {
            external_web_access: None,
            indexed_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }],
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn tool_search_output_stays_on_legacy_path_until_discovery_result_ir_exists() {
    let prompt = prompt_with(
        vec![ResponseItem::ToolSearchOutput {
            id: None,
            call_id: Some("call-search-1".to_string()),
            status: "completed".to_string(),
            execution: TOOL_SEARCH_CLIENT_EXECUTION.to_string(),
            tools: vec![json!({
                "type": "function",
                "name": "discovered_tool",
                "description": "A Responses-shaped discovered tool",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "strict": false
            })],
            internal_chat_message_metadata_passthrough: None,
        }],
        Vec::new(),
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn function_call_and_text_result_round_trip_preserves_argument_bytes_and_metadata() {
    let metadata = InternalChatMessageMetadataPassthrough {
        turn_id: Some("turn-tool".to_string()),
        ..Default::default()
    };
    let arguments = "{ \"path\" : \"README.md\" }".to_string();
    let prompt = prompt_with(
        vec![
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::with_suffix("fc", "request-bridge")),
                name: "read_file".to_string(),
                namespace: Some("workspace".to_string()),
                arguments: arguments.clone(),
                encrypted_function_args: Some(vec!["private-continuation".to_string()]),
                call_id: "call-1".to_string(),
                internal_chat_message_metadata_passthrough: Some(metadata.clone()),
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::with_suffix("fco", "request-bridge")),
                call_id: Some("call-1".to_string()),
                name: Some("read_file".to_string()),
                namespace: Some("workspace".to_string()),
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text("contents".to_string()),
                    success: Some(true),
                },
                internal_chat_message_metadata_passthrough: Some(metadata),
            },
        ],
        Vec::new(),
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    let ModelInputItem::ToolCall(call) = &request.input[0] else {
        panic!("expected canonical tool call");
    };
    assert_eq!(call.call_id.0, "call-1");
    assert_eq!(
        call.input,
        ModelToolInput::Json(json!({"path": "README.md"}))
    );

    let rebuilt = prompt_from_model_request(&request, &prompt).expect("round trip");
    let ResponseItem::FunctionCall {
        arguments: rebuilt_arguments,
        encrypted_function_args,
        ..
    } = &rebuilt.input[0]
    else {
        panic!("expected function call");
    };
    assert_eq!(rebuilt_arguments, &arguments);
    assert_eq!(
        encrypted_function_args.as_deref(),
        Some(["private-continuation".to_string()].as_slice())
    );
}

#[test]
fn encrypted_tool_result_content_stays_on_legacy_path() {
    let prompt = prompt_with(
        vec![ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-1".to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "opaque".to_string(),
                },
            ]),
            internal_chat_message_metadata_passthrough: None,
        }],
        Vec::new(),
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn responses_encrypted_tool_schema_stays_on_legacy_path() {
    let parameters: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "secret": {
                "type": "string",
                "encrypted": true
            }
        },
        "required": ["secret"],
        "additionalProperties": false
    }))
    .expect("valid Responses schema");

    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Function(ResponsesApiTool {
            name: "reviewed_secret_tool".to_string(),
            description: "Uses a provider-private reviewed parameter".to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })],
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn function_tool_output_schema_stays_on_legacy_path_until_ir_supports_it() {
    let parameters: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    }))
    .expect("valid schema");

    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Function(ResponsesApiTool {
            name: "structured_result_tool".to_string(),
            description: "Has a harness-owned output contract".to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "result": {"type": "string"}
                },
                "required": ["result"],
                "additionalProperties": false
            })),
        })],
    );

    assert_eq!(try_model_request_from_prompt(&prompt), None);
}

#[test]
fn unconstrained_freeform_input_is_rejected_by_current_codex_adapter() {
    let prompt = prompt_with(Vec::new(), Vec::new());
    let mut request = try_model_request_from_prompt(&prompt).expect("empty prompt is canonical");
    request.tools.push(ModelToolSpec::Freeform {
        namespace: None,
        name: "raw".to_string(),
        description: "Raw input".to_string(),
        input_format: ModelFreeformInputFormat::Text,
        availability: ModelToolAvailability::Immediate,
        purpose: ModelToolPurpose::Invocation,
    });

    assert!(prompt_from_model_request(&request, &prompt).is_err());
}
