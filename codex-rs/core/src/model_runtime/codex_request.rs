//! Transitional conversion between Codex's Responses-shaped request payload and Tachyon's
//! canonical model request IR.
//!
//! This module is intentionally Codex-specific. It lets the regular sampling path exercise the
//! canonical `ModelRequest` without promoting provider-private history decorations or Responses
//! wire shapes into the kernel IR. Unsupported request shapes stay on the explicit legacy path.

use std::sync::Arc;

use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use codex_tools::default_namespace_description;
use serde_json::Value;

use crate::client_common::Prompt;
use crate::model_runtime::ir::ModelContent;
use crate::model_runtime::ir::ModelFreeformInputFormat;
use crate::model_runtime::ir::ModelImageDetail;
use crate::model_runtime::ir::ModelInputItem;
use crate::model_runtime::ir::ModelMediaSource;
use crate::model_runtime::ir::ModelMessage;
use crate::model_runtime::ir::ModelMessagePhase;
use crate::model_runtime::ir::ModelMessageRole;
use crate::model_runtime::ir::ModelOutputConfig;
use crate::model_runtime::ir::ModelOutputFormat;
use crate::model_runtime::ir::ModelReasoning;
use crate::model_runtime::ir::ModelRequest;
use crate::model_runtime::ir::ModelToolAvailability;
use crate::model_runtime::ir::ModelToolCall;
use crate::model_runtime::ir::ModelToolCallId;
use crate::model_runtime::ir::ModelToolInput;
use crate::model_runtime::ir::ModelToolPurpose;
use crate::model_runtime::ir::ModelToolResult;
use crate::model_runtime::ir::ModelToolResultContent;
use crate::model_runtime::ir::ModelToolSpec;

const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_CLIENT_EXECUTION: &str = "client";
const FREEFORM_GRAMMAR_FORMAT: &str = "grammar";

/// Returns a canonical request only when the current Codex prompt can be represented without
/// losing model-visible semantics.
///
/// Provider-private item IDs and internal passthrough metadata are deliberately not copied into
/// `ModelRequest`. The reverse conversion receives the original `Prompt` as a migration-only
/// template and restores those decorations below the model-runtime boundary.
pub(super) fn try_model_request_from_prompt(prompt: &Prompt) -> Option<ModelRequest> {
    // Access programs are an OpenAI/Codex request authorization mechanism. Keep requests that use
    // them on the legacy path until a provider-neutral capability boundary is justified.
    if prompt.cyber_access_program.is_some() {
        return None;
    }

    let input = prompt
        .input
        .iter()
        .map(model_input_item_from_response)
        .collect::<Option<Vec<_>>>()?;
    let tools = model_tools_from_codex(prompt.tools.as_ref())?;
    let output = match &prompt.output_schema {
        Some(schema) => ModelOutputConfig {
            format: ModelOutputFormat::JsonSchema {
                schema: schema.clone(),
                strict: prompt.output_schema_strict,
            },
        },
        None => ModelOutputConfig {
            format: ModelOutputFormat::Text,
        },
    };

    let request = ModelRequest {
        instructions: prompt.base_instructions.text.clone(),
        input,
        tools,
        parallel_tool_calls: prompt.parallel_tool_calls,
        output,
    };

    // C2 is a behavior-preserving migration bridge. Only take the canonical
    // path when converting back through the current Codex adapter reproduces
    // the complete transitional Prompt exactly. Anything else stays on the
    // legacy path instead of turning a migration mismatch into a user-facing
    // InvalidRequest.
    let rebuilt = prompt_from_model_request(&request, prompt).ok()?;
    if !prompt_round_trip_matches(&rebuilt, prompt) {
        return None;
    }

    Some(request)
}

fn prompt_round_trip_matches(rebuilt: &Prompt, original: &Prompt) -> bool {
    rebuilt.input == original.input
        && rebuilt.tools.as_ref() == original.tools.as_ref()
        && rebuilt.parallel_tool_calls == original.parallel_tool_calls
        && rebuilt.base_instructions == original.base_instructions
        && rebuilt.output_schema == original.output_schema
        && rebuilt.output_schema_strict == original.output_schema_strict
        && rebuilt.cyber_access_program == original.cyber_access_program
}

/// Reconstructs the current Codex prompt from canonical semantics while using the legacy prompt
/// only as a provider-private decoration template during migration.
///
/// The canonical request is produced immediately before this call in the C2 path, so the legacy
/// template has the same item layout. Keeping the template here preserves Responses item IDs,
/// internal passthrough metadata, encrypted function arguments, and other Codex-private details
/// without adding them to the canonical IR.
pub(super) fn prompt_from_model_request(
    request: &ModelRequest,
    legacy_prompt: &Prompt,
) -> Result<Prompt> {
    if legacy_prompt.cyber_access_program.is_some() {
        return Err(invalid_request(
            "canonical request conversion does not support Codex access programs",
        ));
    }
    if request.input.len() != legacy_prompt.input.len() {
        return Err(invalid_request(
            "canonical request input no longer matches the migration template",
        ));
    }

    let input = request
        .input
        .iter()
        .zip(&legacy_prompt.input)
        .map(|(item, legacy)| response_item_from_model_input(item, legacy))
        .collect::<Result<Vec<_>>>()?;
    let tools = codex_tools_from_model(&request.tools)?;

    let (output_schema, output_schema_strict) = match &request.output.format {
        ModelOutputFormat::Text => (None, legacy_prompt.output_schema_strict),
        ModelOutputFormat::JsonSchema { schema, strict } => (Some(schema.clone()), *strict),
    };

    Ok(Prompt {
        input,
        tools: Arc::from(tools),
        parallel_tool_calls: request.parallel_tool_calls,
        base_instructions: BaseInstructions {
            text: request.instructions.clone(),
            provenance: legacy_prompt.base_instructions.provenance.clone(),
        },
        output_schema,
        output_schema_strict,
        cyber_access_program: None,
    })
}

fn model_input_item_from_response(item: &ResponseItem) -> Option<ModelInputItem> {
    match item {
        ResponseItem::Message {
            role,
            content,
            phase,
            ..
        } => Some(ModelInputItem::Message(ModelMessage {
            role: model_message_role(role)?,
            phase: phase.as_ref().map(model_message_phase),
            content: content
                .iter()
                .map(|content| model_content_from_response(content, role))
                .collect::<Option<Vec<_>>>()?,
        })),
        ResponseItem::Reasoning {
            summary, content, ..
        } => Some(ModelInputItem::Reasoning(ModelReasoning {
            summary: summary
                .iter()
                .map(|summary| match summary {
                    ReasoningItemReasoningSummary::SummaryText { text } => text.clone(),
                })
                .collect(),
            content: content
                .iter()
                .flatten()
                .map(|content| match content {
                    ReasoningItemContent::ReasoningText { text }
                    | ReasoningItemContent::Text { text } => text.clone(),
                })
                .collect(),
        })),
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => Some(ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId(call_id.clone()),
            namespace: namespace.clone(),
            name: name.clone(),
            input: ModelToolInput::Json(serde_json::from_str(arguments).ok()?),
        })),
        ResponseItem::CustomToolCall {
            call_id,
            name,
            namespace,
            input,
            ..
        } => Some(ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId(call_id.clone()),
            namespace: namespace.clone(),
            name: name.clone(),
            input: ModelToolInput::Text(input.clone()),
        })),
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            execution,
            arguments,
            ..
        } if execution == TOOL_SEARCH_CLIENT_EXECUTION => {
            Some(ModelInputItem::ToolCall(ModelToolCall {
                call_id: ModelToolCallId(call_id.clone()),
                namespace: None,
                name: TOOL_SEARCH_NAME.to_string(),
                input: ModelToolInput::Json(arguments.clone()),
            }))
        }
        ResponseItem::FunctionCallOutput {
            call_id: Some(call_id),
            output,
            ..
        }
        | ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => Some(ModelInputItem::ToolResult(ModelToolResult {
            call_id: ModelToolCallId(call_id.clone()),
            content: model_tool_result_content(output)?,
            is_error: output.success == Some(false),
        })),
        // Discovery-result payloads, local shell, built-in provider tools, compaction, agent
        // messaging, and provider-generated compatibility items stay on the explicit legacy path.
        // In particular, ToolSearchOutput currently contains Responses-shaped serialized tool
        // declarations; C2 must not carry them through generic ModelToolResultContent::Json until a
        // provider-neutral discovery-result representation exists.
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => None,
    }
}

fn response_item_from_model_input(
    item: &ModelInputItem,
    legacy: &ResponseItem,
) -> Result<ResponseItem> {
    match (item, legacy) {
        (
            ModelInputItem::Message(message),
            ResponseItem::Message {
                id,
                content: legacy_content,
                internal_chat_message_metadata_passthrough,
                ..
            },
        ) => {
            if message.content.len() != legacy_content.len() {
                return Err(invalid_request(
                    "canonical message content no longer matches the migration template",
                ));
            }
            let role = response_message_role(message.role);
            let content = message
                .content
                .iter()
                .zip(legacy_content)
                .map(|(content, legacy)| response_content_from_model(content, legacy))
                .collect::<Result<Vec<_>>>()?;
            Ok(ResponseItem::Message {
                id: id.clone(),
                role,
                content,
                phase: message.phase.map(response_message_phase),
                internal_chat_message_metadata_passthrough:
                    internal_chat_message_metadata_passthrough.clone(),
            })
        }
        (ModelInputItem::Reasoning(reasoning), legacy @ ResponseItem::Reasoning { .. }) => {
            let Some(ModelInputItem::Reasoning(expected)) = model_input_item_from_response(legacy)
            else {
                return Err(invalid_request(
                    "reasoning migration template is no longer canonicalizable",
                ));
            };
            if &expected != reasoning {
                return Err(invalid_request(
                    "canonical reasoning no longer matches the migration template",
                ));
            }
            Ok(legacy.clone())
        }
        (
            ModelInputItem::ToolCall(call),
            ResponseItem::FunctionCall {
                id,
                arguments,
                encrypted_function_args,
                internal_chat_message_metadata_passthrough,
                ..
            },
        ) => {
            let ModelToolInput::Json(value) = &call.input else {
                return Err(invalid_request(
                    "function-call migration template requires structured JSON input",
                ));
            };
            // Preserve the original JSON text when it represents the same canonical value. This
            // avoids changing request bytes and websocket incremental-request comparisons merely
            // because serde_json would choose different whitespace.
            let arguments = match serde_json::from_str::<Value>(arguments) {
                Ok(original) if &original == value => arguments.clone(),
                _ => {
                    serde_json::to_string(value).map_err(|err| invalid_request(err.to_string()))?
                }
            };
            Ok(ResponseItem::FunctionCall {
                id: id.clone(),
                name: call.name.clone(),
                namespace: call.namespace.clone(),
                arguments,
                encrypted_function_args: encrypted_function_args.clone(),
                call_id: call.call_id.0.clone(),
                internal_chat_message_metadata_passthrough:
                    internal_chat_message_metadata_passthrough.clone(),
            })
        }
        (
            ModelInputItem::ToolCall(call),
            ResponseItem::CustomToolCall {
                id,
                status,
                internal_chat_message_metadata_passthrough,
                ..
            },
        ) => {
            let ModelToolInput::Text(input) = &call.input else {
                return Err(invalid_request(
                    "custom-tool migration template requires textual input",
                ));
            };
            Ok(ResponseItem::CustomToolCall {
                id: id.clone(),
                status: status.clone(),
                call_id: call.call_id.0.clone(),
                name: call.name.clone(),
                namespace: call.namespace.clone(),
                input: input.clone(),
                internal_chat_message_metadata_passthrough:
                    internal_chat_message_metadata_passthrough.clone(),
            })
        }
        (
            ModelInputItem::ToolCall(call),
            ResponseItem::ToolSearchCall {
                id,
                call_id,
                status,
                execution,
                internal_chat_message_metadata_passthrough,
                ..
            },
        ) if call.name == TOOL_SEARCH_NAME
            && call.namespace.is_none()
            && execution == TOOL_SEARCH_CLIENT_EXECUTION =>
        {
            let ModelToolInput::Json(arguments) = &call.input else {
                return Err(invalid_request(
                    "tool-discovery migration template requires structured JSON input",
                ));
            };
            Ok(ResponseItem::ToolSearchCall {
                id: id.clone(),
                call_id: Some(call.call_id.0.clone()).or_else(|| call_id.clone()),
                status: status.clone(),
                execution: execution.clone(),
                arguments: arguments.clone(),
                internal_chat_message_metadata_passthrough:
                    internal_chat_message_metadata_passthrough.clone(),
            })
        }
        (ModelInputItem::ToolResult(result), legacy @ ResponseItem::FunctionCallOutput { .. })
        | (
            ModelInputItem::ToolResult(result),
            legacy @ ResponseItem::CustomToolCallOutput { .. },
        )
        | (ModelInputItem::ToolResult(result), legacy @ ResponseItem::ToolSearchOutput { .. }) => {
            // For C2 the canonical request is produced immediately from this same template. Validate
            // that the generic semantics are unchanged, then keep the exact provider-private
            // output encoding (including item IDs, metadata, and encrypted/private decorations).
            let Some(ModelInputItem::ToolResult(expected)) = model_input_item_from_response(legacy)
            else {
                return Err(invalid_request(
                    "tool-result migration template is no longer canonicalizable",
                ));
            };
            if &expected != result {
                return Err(invalid_request(
                    "canonical tool result no longer matches the migration template",
                ));
            }
            Ok(legacy.clone())
        }
        _ => Err(invalid_request(
            "canonical request item no longer matches the migration template",
        )),
    }
}

fn model_message_role(role: &str) -> Option<ModelMessageRole> {
    match role {
        "system" => Some(ModelMessageRole::System),
        "developer" => Some(ModelMessageRole::Developer),
        "user" => Some(ModelMessageRole::User),
        "assistant" => Some(ModelMessageRole::Assistant),
        _ => None,
    }
}

fn response_message_role(role: ModelMessageRole) -> String {
    match role {
        ModelMessageRole::System => "system",
        ModelMessageRole::Developer => "developer",
        ModelMessageRole::User => "user",
        ModelMessageRole::Assistant => "assistant",
    }
    .to_string()
}

fn model_message_phase(phase: &MessagePhase) -> ModelMessagePhase {
    match phase {
        MessagePhase::Commentary => ModelMessagePhase::Commentary,
        MessagePhase::FinalAnswer => ModelMessagePhase::Final,
    }
}

fn response_message_phase(phase: ModelMessagePhase) -> MessagePhase {
    match phase {
        ModelMessagePhase::Commentary => MessagePhase::Commentary,
        ModelMessagePhase::Final => MessagePhase::FinalAnswer,
    }
}

fn model_content_from_response(content: &ContentItem, role: &str) -> Option<ModelContent> {
    match content {
        ContentItem::InputText { text } if role != "assistant" => {
            Some(ModelContent::Text(text.clone()))
        }
        ContentItem::OutputText { text } if role == "assistant" => {
            Some(ModelContent::Text(text.clone()))
        }
        ContentItem::InputImage { image_url, detail } => Some(ModelContent::Image {
            source: ModelMediaSource::Uri(image_url.clone()),
            detail: detail.map(model_image_detail),
        }),
        ContentItem::InputAudio { audio_url } => Some(ModelContent::Audio {
            source: ModelMediaSource::Uri(audio_url.clone()),
        }),
        ContentItem::InputText { .. } | ContentItem::OutputText { .. } => None,
    }
}

fn response_content_from_model(
    content: &ModelContent,
    legacy: &ContentItem,
) -> Result<ContentItem> {
    match (content, legacy) {
        (ModelContent::Text(text), ContentItem::InputText { .. }) => {
            Ok(ContentItem::InputText { text: text.clone() })
        }
        (ModelContent::Text(text), ContentItem::OutputText { .. }) => {
            Ok(ContentItem::OutputText { text: text.clone() })
        }
        (ModelContent::Image { source, detail }, ContentItem::InputImage { .. }) => {
            Ok(ContentItem::InputImage {
                image_url: codex_media_source(source)?,
                detail: detail.map(response_image_detail),
            })
        }
        (ModelContent::Audio { source }, ContentItem::InputAudio { .. }) => {
            Ok(ContentItem::InputAudio {
                audio_url: codex_media_source(source)?,
            })
        }
        _ => Err(invalid_request(
            "canonical content no longer matches the migration template",
        )),
    }
}

fn model_image_detail(detail: ImageDetail) -> ModelImageDetail {
    match detail {
        ImageDetail::Auto => ModelImageDetail::Auto,
        ImageDetail::Low => ModelImageDetail::Low,
        ImageDetail::High => ModelImageDetail::High,
        ImageDetail::Original => ModelImageDetail::Original,
    }
}

fn response_image_detail(detail: ModelImageDetail) -> ImageDetail {
    match detail {
        ModelImageDetail::Auto => ImageDetail::Auto,
        ModelImageDetail::Low => ImageDetail::Low,
        ModelImageDetail::High => ImageDetail::High,
        ModelImageDetail::Original => ImageDetail::Original,
    }
}

fn codex_media_source(source: &ModelMediaSource) -> Result<String> {
    match source {
        ModelMediaSource::Uri(uri) => Ok(uri.clone()),
        ModelMediaSource::Bytes { media_type, data } => Ok(codex_utils_image::data_url_from_bytes(
            media_type,
            data.as_ref(),
        )),
    }
}

fn model_tool_result_content(
    output: &FunctionCallOutputPayload,
) -> Option<Vec<ModelToolResultContent>> {
    match &output.body {
        FunctionCallOutputBody::Text(text) => {
            Some(vec![ModelToolResultContent::Text(text.clone())])
        }
        FunctionCallOutputBody::ContentItems(items) => items
            .iter()
            .map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => {
                    Some(ModelToolResultContent::Text(text.clone()))
                }
                FunctionCallOutputContentItem::InputImage { image_url, detail } => {
                    Some(ModelToolResultContent::Image {
                        source: ModelMediaSource::Uri(image_url.clone()),
                        detail: detail.map(model_image_detail),
                    })
                }
                FunctionCallOutputContentItem::InputAudio { audio_url } => {
                    Some(ModelToolResultContent::Audio {
                        source: ModelMediaSource::Uri(audio_url.clone()),
                    })
                }
                FunctionCallOutputContentItem::EncryptedContent { .. } => None,
            })
            .collect(),
    }
}

fn schema_has_responses_encrypted_marker(schema: &codex_tools::JsonSchema) -> bool {
    if schema.encrypted.is_some() {
        return true;
    }

    schema
        .items
        .as_deref()
        .is_some_and(schema_has_responses_encrypted_marker)
        || schema.properties.as_ref().is_some_and(|properties| {
            properties
                .values()
                .any(schema_has_responses_encrypted_marker)
        })
        || schema
            .additional_properties
            .as_ref()
            .is_some_and(|additional| match additional {
                codex_tools::AdditionalProperties::Boolean(_) => false,
                codex_tools::AdditionalProperties::Schema(schema) => {
                    schema_has_responses_encrypted_marker(schema)
                }
            })
        || schema
            .any_of
            .as_ref()
            .is_some_and(|schemas| schemas.iter().any(schema_has_responses_encrypted_marker))
        || schema
            .one_of
            .as_ref()
            .is_some_and(|schemas| schemas.iter().any(schema_has_responses_encrypted_marker))
        || schema
            .all_of
            .as_ref()
            .is_some_and(|schemas| schemas.iter().any(schema_has_responses_encrypted_marker))
        || schema
            .defs
            .as_ref()
            .is_some_and(|schemas| schemas.values().any(schema_has_responses_encrypted_marker))
        || schema
            .definitions
            .as_ref()
            .is_some_and(|schemas| schemas.values().any(schema_has_responses_encrypted_marker))
}

fn model_tools_from_codex(tools: &[ToolSpec]) -> Option<Vec<ModelToolSpec>> {
    let mut model_tools = Vec::new();
    for tool in tools {
        match tool {
            ToolSpec::Function(tool) => {
                model_tools.push(model_function_tool(
                    None,
                    tool,
                    ModelToolPurpose::Invocation,
                )?);
            }
            ToolSpec::Freeform(tool) => {
                model_tools.push(model_freeform_tool(
                    None,
                    tool,
                    ModelToolPurpose::Invocation,
                )?);
            }
            ToolSpec::Namespace(namespace) => {
                if namespace.description != default_namespace_description(&namespace.name) {
                    return None;
                }
                for tool in &namespace.tools {
                    match tool {
                        ResponsesApiNamespaceTool::Function(tool) => {
                            model_tools.push(model_function_tool(
                                Some(namespace.name.clone()),
                                tool,
                                ModelToolPurpose::Invocation,
                            )?)
                        }
                        ResponsesApiNamespaceTool::Custom(tool) => {
                            model_tools.push(model_freeform_tool(
                                Some(namespace.name.clone()),
                                tool,
                                ModelToolPurpose::Invocation,
                            )?)
                        }
                    }
                }
            }
            ToolSpec::ToolSearch {
                execution,
                description,
                parameters,
            } if execution == TOOL_SEARCH_CLIENT_EXECUTION => {
                if schema_has_responses_encrypted_marker(parameters) {
                    return None;
                }
                model_tools.push(ModelToolSpec::Function {
                    namespace: None,
                    name: TOOL_SEARCH_NAME.to_string(),
                    description: description.clone(),
                    input_schema: serde_json::to_value(parameters).ok()?,
                    strict: false,
                    availability: ModelToolAvailability::Immediate,
                    purpose: ModelToolPurpose::Discovery,
                });
            }
            ToolSpec::ToolSearch { .. } | ToolSpec::WebSearch { .. } => return None,
        }
    }
    Some(model_tools)
}

fn model_function_tool(
    namespace: Option<String>,
    tool: &ResponsesApiTool,
    purpose: ModelToolPurpose,
) -> Option<ModelToolSpec> {
    // `JsonSchema::encrypted` is a Responses-only reviewed-parameter marker,
    // not JSON Schema or provider-neutral model semantics. Keep such tool
    // declarations on the legacy request path during C2.
    if schema_has_responses_encrypted_marker(&tool.parameters) {
        return None;
    }

    Some(ModelToolSpec::Function {
        namespace,
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: serde_json::to_value(&tool.parameters).ok()?,
        strict: tool.strict,
        availability: model_tool_availability(tool.defer_loading)?,
        purpose,
    })
}

fn model_freeform_tool(
    namespace: Option<String>,
    tool: &FreeformTool,
    purpose: ModelToolPurpose,
) -> Option<ModelToolSpec> {
    if tool.format.r#type != FREEFORM_GRAMMAR_FORMAT {
        return None;
    }
    Some(ModelToolSpec::Freeform {
        namespace,
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_format: ModelFreeformInputFormat::Grammar {
            syntax: tool.format.syntax.clone(),
            definition: tool.format.definition.clone(),
        },
        availability: model_tool_availability(tool.defer_loading)?,
        purpose,
    })
}

fn model_tool_availability(defer_loading: Option<bool>) -> Option<ModelToolAvailability> {
    match defer_loading {
        None => Some(ModelToolAvailability::Immediate),
        Some(true) => Some(ModelToolAvailability::Deferred),
        Some(false) => None,
    }
}

fn codex_tools_from_model(tools: &[ModelToolSpec]) -> Result<Vec<ToolSpec>> {
    let mut output = Vec::new();
    for tool in tools {
        let namespace = match tool {
            ModelToolSpec::Function { namespace, .. }
            | ModelToolSpec::Freeform { namespace, .. } => namespace.as_deref(),
        };
        if let Some(namespace) = namespace {
            let namespace_tool = codex_namespace_tool_from_model(tool)?;
            if let Some(ToolSpec::Namespace(existing)) = output.last_mut()
                && existing.name == namespace
            {
                existing.tools.push(namespace_tool);
            } else {
                output.push(ToolSpec::Namespace(ResponsesApiNamespace {
                    name: namespace.to_string(),
                    description: default_namespace_description(namespace),
                    tools: vec![namespace_tool],
                }));
            }
            continue;
        }
        output.push(codex_root_tool_from_model(tool)?);
    }
    Ok(output)
}

fn codex_namespace_tool_from_model(tool: &ModelToolSpec) -> Result<ResponsesApiNamespaceTool> {
    match tool {
        ModelToolSpec::Function {
            purpose: ModelToolPurpose::Invocation,
            ..
        } => Ok(ResponsesApiNamespaceTool::Function(codex_function_tool(
            tool,
        )?)),
        ModelToolSpec::Freeform {
            purpose: ModelToolPurpose::Invocation,
            ..
        } => Ok(ResponsesApiNamespaceTool::Custom(codex_freeform_tool(
            tool,
        )?)),
        ModelToolSpec::Function { .. } | ModelToolSpec::Freeform { .. } => Err(invalid_request(
            "discovery tools cannot be nested in a Codex tool namespace",
        )),
    }
}

fn codex_root_tool_from_model(tool: &ModelToolSpec) -> Result<ToolSpec> {
    match tool {
        ModelToolSpec::Function {
            name,
            input_schema,
            strict,
            availability,
            purpose: ModelToolPurpose::Discovery,
            ..
        } if name == TOOL_SEARCH_NAME
            && !*strict
            && *availability == ModelToolAvailability::Immediate =>
        {
            Ok(ToolSpec::ToolSearch {
                execution: TOOL_SEARCH_CLIENT_EXECUTION.to_string(),
                description: tool_description(tool).to_string(),
                parameters: serde_json::from_value(input_schema.clone())
                    .map_err(|err| invalid_request(err.to_string()))?,
            })
        }
        ModelToolSpec::Function {
            purpose: ModelToolPurpose::Invocation,
            ..
        } => Ok(ToolSpec::Function(codex_function_tool(tool)?)),
        ModelToolSpec::Freeform {
            purpose: ModelToolPurpose::Invocation,
            ..
        } => Ok(ToolSpec::Freeform(codex_freeform_tool(tool)?)),
        ModelToolSpec::Function { .. } | ModelToolSpec::Freeform { .. } => Err(invalid_request(
            "canonical discovery tool cannot be represented by the Codex adapter",
        )),
    }
}

fn codex_function_tool(tool: &ModelToolSpec) -> Result<ResponsesApiTool> {
    let ModelToolSpec::Function {
        name,
        description,
        input_schema,
        strict,
        availability,
        purpose: ModelToolPurpose::Invocation,
        ..
    } = tool
    else {
        return Err(invalid_request("expected an invocation function tool"));
    };
    Ok(ResponsesApiTool {
        name: name.clone(),
        description: description.clone(),
        strict: *strict,
        defer_loading: codex_defer_loading(*availability),
        parameters: serde_json::from_value(input_schema.clone())
            .map_err(|err| invalid_request(err.to_string()))?,
        output_schema: None,
    })
}

fn codex_freeform_tool(tool: &ModelToolSpec) -> Result<FreeformTool> {
    let ModelToolSpec::Freeform {
        name,
        description,
        input_format,
        availability,
        purpose: ModelToolPurpose::Invocation,
        ..
    } = tool
    else {
        return Err(invalid_request("expected an invocation free-form tool"));
    };
    let ModelFreeformInputFormat::Grammar { syntax, definition } = input_format else {
        return Err(invalid_request(
            "the current Codex adapter cannot represent unconstrained free-form tool input",
        ));
    };
    Ok(FreeformTool {
        name: name.clone(),
        description: description.clone(),
        defer_loading: codex_defer_loading(*availability),
        format: FreeformToolFormat {
            r#type: FREEFORM_GRAMMAR_FORMAT.to_string(),
            syntax: syntax.clone(),
            definition: definition.clone(),
        },
    })
}

fn tool_description(tool: &ModelToolSpec) -> &str {
    match tool {
        ModelToolSpec::Function { description, .. }
        | ModelToolSpec::Freeform { description, .. } => description,
    }
}

fn codex_defer_loading(availability: ModelToolAvailability) -> Option<bool> {
    match availability {
        ModelToolAvailability::Immediate => None,
        ModelToolAvailability::Deferred => Some(true),
    }
}

fn invalid_request(message: impl Into<String>) -> codex_protocol::error::CodexErr {
    CodexErrorDetails::InvalidRequest(message.into()).into()
}

#[cfg(test)]
#[path = "codex_request_tests.rs"]
mod tests;
