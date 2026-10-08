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
fn client_discovery_output_keeps_explicit_groups_out_of_its_flat_vocabulary() {
    let group = ModelToolSpec::Namespace {
        name: "workspace".to_string(),
        description: "Custom guidance".to_string(),
        tools: Vec::new(),
    };

    assert_eq!(tool_search_output_values_from_model(&[group]), None);
}

#[test]
fn client_discovery_output_rejects_function_output_schemas_without_changing_none_case() {
    let function = |output_schema| ModelToolSpec::Function {
        namespace: None,
        name: "discovered_tool".to_string(),
        description: "A discovered tool".to_string(),
        input_schema: json!({"type": "object"}),
        output_schema,
        strict: false,
        availability: ModelToolAvailability::Immediate,
        purpose: ModelToolPurpose::Invocation,
    };

    assert_eq!(
        tool_search_output_values_from_model(&[function(Some(json!({"type": "string"})))]),
        None
    );
    assert!(tool_search_output_values_from_model(&[function(None)]).is_some());
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
        [ModelToolSpec::Namespace {
            name: namespace,
            description,
            tools,
        }] if namespace == "workspace"
            && *description == default_namespace_description("workspace")
            && matches!(tools.as_slice(), [ModelToolSpec::Function {
                namespace: None,
                name,
                output_schema: None,
                ..
            }] if name == "read_file")
    ));
}

#[test]
fn custom_namespace_description_and_empty_group_round_trip() {
    let prompt = prompt_with(
        Vec::new(),
        vec![ToolSpec::Namespace(ResponsesApiNamespace {
            name: "workspace".to_string(),
            description: "Custom namespace guidance".to_string(),
            tools: Vec::new(),
        })],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert!(matches!(
        request.tools.as_slice(),
        [ModelToolSpec::Namespace { name, description, tools }]
            if name == "workspace"
                && description == "Custom namespace guidance"
                && tools.is_empty()
    ));
}

#[test]
fn namespace_groups_preserve_order_duplicates_mixed_children_and_output_schemas() {
    let schema: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    }))
    .expect("valid schema");
    let output_schema = json!({
        "type": "object",
        "properties": {"result": {"type": "string"}},
        "required": ["result"],
        "additionalProperties": false
    });
    let prompt = prompt_with(
        Vec::new(),
        vec![
            ToolSpec::Function(ResponsesApiTool {
                name: "root_tool".to_string(),
                description: "Root tool".to_string(),
                strict: false,
                defer_loading: None,
                parameters: schema.clone(),
                output_schema: None,
            }),
            ToolSpec::Namespace(ResponsesApiNamespace {
                name: "workspace".to_string(),
                description: "First custom description".to_string(),
                tools: vec![
                    ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                        name: "read_file".to_string(),
                        description: "Read a file".to_string(),
                        strict: true,
                        defer_loading: None,
                        parameters: schema.clone(),
                        output_schema: Some(output_schema.clone()),
                    }),
                    ResponsesApiNamespaceTool::Custom(FreeformTool {
                        name: "apply_patch".to_string(),
                        description: "Apply a patch".to_string(),
                        defer_loading: Some(true),
                        format: FreeformToolFormat {
                            r#type: FREEFORM_GRAMMAR_FORMAT.to_string(),
                            syntax: "lark".to_string(),
                            definition: "start: patch".to_string(),
                        },
                    }),
                ],
            }),
            ToolSpec::Namespace(ResponsesApiNamespace {
                name: "workspace".to_string(),
                description: "Second custom description".to_string(),
                tools: Vec::new(),
            }),
            ToolSpec::Namespace(ResponsesApiNamespace {
                name: "workspace".to_string(),
                description: "Third custom description".to_string(),
                tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                    name: "write_file".to_string(),
                    description: "Write a file".to_string(),
                    strict: false,
                    defer_loading: Some(true),
                    parameters: schema,
                    output_schema: None,
                })],
            }),
        ],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    let [
        ModelToolSpec::Function {
            namespace: None,
            name: root_name,
            output_schema: None,
            ..
        },
        ModelToolSpec::Namespace {
            name: first_name,
            description: first_description,
            tools: first_tools,
        },
        ModelToolSpec::Namespace {
            name: second_name,
            description: second_description,
            tools: second_tools,
        },
        ModelToolSpec::Namespace {
            name: third_name,
            description: third_description,
            tools: third_tools,
        },
    ] = request.tools.as_slice()
    else {
        panic!("expected root tool followed by three distinct namespace groups");
    };
    assert_eq!(root_name, "root_tool");
    assert_eq!(first_name, "workspace");
    assert_eq!(first_description, "First custom description");
    assert_eq!(second_name, "workspace");
    assert_eq!(second_description, "Second custom description");
    assert!(second_tools.is_empty());
    assert_eq!(third_name, "workspace");
    assert_eq!(third_description, "Third custom description");

    assert!(matches!(
        first_tools.as_slice(),
        [
            ModelToolSpec::Function {
                namespace: None,
                name,
                output_schema: Some(schema),
                ..
            },
            ModelToolSpec::Freeform {
                namespace: None,
                name: freeform_name,
                availability: ModelToolAvailability::Deferred,
                ..
            }
        ] if name == "read_file" && schema == &output_schema && freeform_name == "apply_patch"
    ));
    assert!(matches!(
        third_tools.as_slice(),
        [ModelToolSpec::Function {
            namespace: None,
            name,
            output_schema: None,
            availability: ModelToolAvailability::Deferred,
            ..
        }] if name == "write_file"
    ));
}

#[test]
fn legacy_flat_namespace_leaves_keep_grouping_without_merging_explicit_groups() {
    let prompt = prompt_with(Vec::new(), Vec::new());
    let mut request = try_model_request_from_prompt(&prompt).expect("empty prompt is canonical");
    let input_schema = json!({"type": "object", "properties": {}});
    let function = |name: &str, output_schema: Option<serde_json::Value>| ModelToolSpec::Function {
        namespace: Some("workspace".to_string()),
        name: name.to_string(),
        description: name.to_string(),
        input_schema: input_schema.clone(),
        output_schema,
        strict: false,
        availability: ModelToolAvailability::Immediate,
        purpose: ModelToolPurpose::Invocation,
    };
    request.tools = vec![
        ModelToolSpec::Namespace {
            name: "workspace".to_string(),
            description: "Explicit one".to_string(),
            tools: Vec::new(),
        },
        function("flat_one", None),
        function("flat_two", Some(json!({"type": "string"}))),
        ModelToolSpec::Namespace {
            name: "workspace".to_string(),
            description: "Explicit two".to_string(),
            tools: Vec::new(),
        },
        function("flat_three", None),
    ];

    let rebuilt = prompt_from_model_request(&request, &prompt).expect("flat leaves rebuild");
    let [
        ToolSpec::Namespace(first_explicit),
        ToolSpec::Namespace(first_flat_group),
        ToolSpec::Namespace(second_explicit),
        ToolSpec::Namespace(second_flat_group),
    ] = rebuilt.tools.as_ref()
    else {
        panic!("expected explicit groups and separately coalesced flat groups");
    };
    assert_eq!(first_explicit.description, "Explicit one");
    assert!(first_explicit.tools.is_empty());
    assert_eq!(
        first_flat_group.description,
        default_namespace_description("workspace")
    );
    assert!(matches!(
        first_flat_group.tools.as_slice(),
        [
            ResponsesApiNamespaceTool::Function(first),
            ResponsesApiNamespaceTool::Function(second)
        ] if first.name == "flat_one"
            && first.output_schema.is_none()
            && second.name == "flat_two"
            && second.output_schema == Some(json!({"type": "string"}))
    ));
    assert_eq!(second_explicit.description, "Explicit two");
    assert!(second_explicit.tools.is_empty());
    assert_eq!(
        second_flat_group.description,
        default_namespace_description("workspace")
    );
    assert!(matches!(
        second_flat_group.tools.as_slice(),
        [ResponsesApiNamespaceTool::Function(tool)] if tool.name == "flat_three"
    ));
}

#[test]
fn explicit_namespace_rejects_nested_discovery_and_conflicting_children() {
    let prompt = prompt_with(Vec::new(), Vec::new());
    let request = || try_model_request_from_prompt(&prompt).expect("empty prompt is canonical");
    let group = |tools| ModelToolSpec::Namespace {
        name: "workspace".to_string(),
        description: "Workspace".to_string(),
        tools,
    };
    let function = |namespace, purpose| ModelToolSpec::Function {
        namespace,
        name: "tool".to_string(),
        description: "Tool".to_string(),
        input_schema: json!({"type": "object"}),
        output_schema: None,
        strict: false,
        availability: ModelToolAvailability::Immediate,
        purpose,
    };

    for invalid_group in [
        group(vec![group(Vec::new())]),
        group(vec![function(
            Some("other".to_string()),
            ModelToolPurpose::Invocation,
        )]),
        group(vec![function(None, ModelToolPurpose::Discovery)]),
    ] {
        let mut invalid_request = request();
        invalid_request.tools = vec![invalid_group];
        assert!(prompt_from_model_request(&invalid_request, &prompt).is_err());
    }
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
fn tool_search_output_round_trips_through_discovery_result_ir() {
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

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    let [ModelInputItem::ToolResult(result)] = request.input.as_slice() else {
        panic!("expected one canonical tool result");
    };
    assert_eq!(result.call_id, ModelToolCallId("call-search-1".to_string()));
    let [ModelToolResultContent::DiscoveredTools(tools)] = result.content.as_slice() else {
        panic!("expected semantic discovered-tool content");
    };
    assert!(matches!(
        tools.as_slice(),
        [ModelToolSpec::Function {
            name,
            purpose: ModelToolPurpose::Invocation,
            ..
        }] if name == "discovered_tool"
    ));
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
fn function_output_schema_round_trips_raw_json_independently_of_input_markers() {
    let parameters: JsonSchema = serde_json::from_value(json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    }))
    .expect("valid schema");
    let output_schema = json!({
        "type": "object",
        "properties": {"result": {"type": "string"}},
        "encrypted": {"provider_marker_like_key": true},
        "required": ["result"],
        "additionalProperties": false
    });

    let prompt = prompt_with(
        Vec::new(),
        vec![
            ToolSpec::Function(ResponsesApiTool {
                name: "structured_result_tool".to_string(),
                description: "Has a harness-owned output contract".to_string(),
                strict: false,
                defer_loading: None,
                parameters: parameters.clone(),
                output_schema: Some(output_schema.clone()),
            }),
            ToolSpec::Function(ResponsesApiTool {
                name: "plain_result_tool".to_string(),
                description: "Has no output contract".to_string(),
                strict: false,
                defer_loading: None,
                parameters,
                output_schema: None,
            }),
        ],
    );

    let request = assert_prompt_request_semantics_round_trip(&prompt);
    assert!(matches!(
        request.tools.as_slice(),
        [
            ModelToolSpec::Function {
                name: with_output,
                output_schema: Some(actual),
                ..
            },
            ModelToolSpec::Function {
                name: without_output,
                output_schema: None,
                ..
            }
        ] if with_output == "structured_result_tool"
            && actual == &output_schema
            && without_output == "plain_result_tool"
    ));
}

#[test]
fn access_program_state_stays_on_legacy_path() {
    let mut prompt = prompt_with(Vec::new(), Vec::new());
    prompt.cyber_access_program = Some(codex_protocol::turn_input::CyberAccessProgram::Standard);

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
