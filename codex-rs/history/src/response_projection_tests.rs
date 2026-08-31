use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn source(item: ResponseItem) -> ResponseItemEnvelope {
    ResponseItemEnvelope::with_metadata(
        item,
        CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(4096),
        },
    )
}

fn canonical(item: ResponseItem, expected: HistoryItem) {
    let source = source(item);
    let (item, compatibility) = match project_response_item(source.clone()) {
        HistoryItemProjection::Canonical {
            item,
            compatibility,
        } => (item, compatibility),
        _ => panic!("expected canonical projection"),
    };
    assert_eq!(item, expected);
    assert_eq!(compatibility, source);
}

fn fallback(item: ResponseItem, expected_reason: HistoryProjectionFallback) {
    let source = source(item);
    let (compatibility, reason) = match project_response_item(source.clone()) {
        HistoryItemProjection::Fallback {
            compatibility,
            reason,
        } => (compatibility, reason),
        _ => panic!("expected fallback projection"),
    };
    assert_eq!(reason, expected_reason);
    assert_eq!(compatibility, source);
}

fn history_message(
    role: HistoryMessageRole,
    phase: Option<HistoryMessagePhase>,
    content: Vec<HistoryMessageContent>,
) -> HistoryItem {
    HistoryItem::Message(HistoryMessage {
        role,
        phase,
        content,
    })
}

fn history_call(
    call_id: &str,
    namespace: Option<&str>,
    name: &str,
    input: HistoryToolInput,
) -> HistoryItem {
    HistoryItem::ToolCall(HistoryToolCall {
        call_id: HistoryToolCallId(call_id.to_string()),
        namespace: namespace.map(str::to_string),
        name: name.to_string(),
        input,
    })
}

fn history_result(
    call_id: &str,
    content: Vec<HistoryToolResultContent>,
    is_error: Option<bool>,
) -> HistoryItem {
    HistoryItem::ToolResult(HistoryToolResult {
        call_id: HistoryToolCallId(call_id.to_string()),
        content,
        is_error,
    })
}

fn message(role: &str, content: Vec<ContentItem>, phase: Option<MessagePhase>) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        phase,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(
    call_id: &str,
    arguments: &str,
    encrypted_function_args: Option<Vec<String>>,
) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: "lookup".to_string(),
        namespace: Some("catalog".to_string()),
        arguments: arguments.to_string(),
        encrypted_function_args,
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn custom_call(call_id: &str, input: &str) -> ResponseItem {
    ResponseItem::CustomToolCall {
        id: None,
        status: Some("completed".to_string()),
        call_id: call_id.to_string(),
        name: "search".to_string(),
        namespace: Some("docs".to_string()),
        input: input.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_result(call_id: Option<&str>, output: FunctionCallOutputPayload) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: call_id.map(str::to_string),
        name: Some("lookup".to_string()),
        namespace: None,
        output,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn custom_result(call_id: &str, output: FunctionCallOutputPayload) -> ResponseItem {
    ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: call_id.to_string(),
        name: None,
        output,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn text_output(text: &str, success: Option<bool>) -> FunctionCallOutputPayload {
    FunctionCallOutputPayload {
        body: FunctionCallOutputBody::Text(text.to_string()),
        success,
    }
}

fn tool_search(call_id: Option<&str>, execution: &str) -> ResponseItem {
    ResponseItem::ToolSearchCall {
        id: None,
        call_id: call_id.map(str::to_string),
        status: Some("completed".to_string()),
        execution: execution.to_string(),
        arguments: json!({"query": "filesystem"}),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn decoded(value: &str) -> ResponseItem {
    serde_json::from_str(value).expect("response item fixture")
}

#[test]
fn known_message_roles_phases_and_content_order_are_projected() {
    canonical(
        decoded(
            r#"{"type":"message","role":"assistant","content":[{"type":"input_text","text":"first"},{"type":"input_image","image_url":"https://example.test/image","detail":"original"},{"type":"input_audio","audio_url":"data:audio/wav;base64,AA=="},{"type":"output_text","text":"last"}],"phase":"final_answer"}"#,
        ),
        history_message(
            HistoryMessageRole::Assistant,
            Some(HistoryMessagePhase::Final),
            vec![
                HistoryMessageContent::Text("first".to_string()),
                HistoryMessageContent::Image {
                    source: HistoryMediaSource::Uri("https://example.test/image".to_string()),
                    detail: Some(HistoryImageDetail::Original),
                },
                HistoryMessageContent::Audio {
                    source: HistoryMediaSource::Uri("data:audio/wav;base64,AA==".to_string()),
                },
                HistoryMessageContent::Text("last".to_string()),
            ],
        ),
    );

    for (role, expected) in [
        ("system", HistoryMessageRole::System),
        ("developer", HistoryMessageRole::Developer),
        ("user", HistoryMessageRole::User),
        ("assistant", HistoryMessageRole::Assistant),
    ] {
        canonical(
            message(role, Vec::new(), Some(MessagePhase::Commentary)),
            history_message(expected, Some(HistoryMessagePhase::Commentary), Vec::new()),
        );
    }
}

#[test]
fn unknown_message_role_retains_exact_compatibility_envelope() {
    fallback(
        message(
            "future_role",
            vec![ContentItem::InputText {
                text: "opaque".to_string(),
            }],
            None,
        ),
        HistoryProjectionFallback::UnknownMessageRole,
    );
}

#[test]
fn reasoning_preserves_summary_and_plaintext_order_but_not_encrypted_content() {
    canonical(
        ResponseItem::Reasoning {
            id: None,
            summary: vec![
                ReasoningItemReasoningSummary::SummaryText {
                    text: "summary one".to_string(),
                },
                ReasoningItemReasoningSummary::SummaryText {
                    text: "summary two".to_string(),
                },
            ],
            content: Some(vec![
                ReasoningItemContent::ReasoningText {
                    text: "plain one".to_string(),
                },
                ReasoningItemContent::Text {
                    text: "plain two".to_string(),
                },
            ]),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        },
        HistoryItem::Reasoning(HistoryReasoning {
            summary: vec!["summary one".to_string(), "summary two".to_string()],
            content: vec!["plain one".to_string(), "plain two".to_string()],
        }),
    );
    fallback(
        ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: None,
            encrypted_content: Some("opaque".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
        HistoryProjectionFallback::EncryptedReasoning,
    );
}

#[test]
fn function_calls_require_plain_valid_json_and_nonempty_call_ids() {
    canonical(
        function_call("call-1", r#"{"query":"rust"}"#, Some(Vec::new())),
        history_call(
            "call-1",
            Some("catalog"),
            "lookup",
            HistoryToolInput::Json(json!({"query": "rust"})),
        ),
    );
    for (item, reason) in [
        (
            function_call("call-1", "not json", None),
            HistoryProjectionFallback::InvalidFunctionArguments,
        ),
        (
            function_call("call-1", "{}", Some(vec!["encrypted".to_string()])),
            HistoryProjectionFallback::EncryptedFunctionArguments,
        ),
        (
            function_call("", "{}", None),
            HistoryProjectionFallback::MissingFunctionCallId,
        ),
    ] {
        fallback(item, reason);
    }
}

#[test]
fn custom_calls_preserve_text_input_and_require_call_id() {
    canonical(
        custom_call("custom-1", "raw text input"),
        history_call(
            "custom-1",
            Some("docs"),
            "search",
            HistoryToolInput::Text("raw text input".to_string()),
        ),
    );
    fallback(
        custom_call("", ""),
        HistoryProjectionFallback::MissingCustomToolCallId,
    );
}

#[test]
fn client_tool_search_is_unambiguous_and_provider_search_falls_back() {
    canonical(
        tool_search(Some("search-1"), "client"),
        history_call(
            "search-1",
            None,
            "tool_search",
            HistoryToolInput::Json(json!({"query": "filesystem"})),
        ),
    );
    for (item, reason) in [
        (
            tool_search(Some("server-search"), "server"),
            HistoryProjectionFallback::ProviderToolSearchCall,
        ),
        (
            tool_search(None, "client"),
            HistoryProjectionFallback::MissingClientToolSearchCallId,
        ),
    ] {
        fallback(item, reason);
    }
}

#[test]
fn tool_results_preserve_content_order_and_invert_all_success_states() {
    canonical(
        function_result(
            Some("call-1"),
            FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "text".to_string(),
                    },
                    FunctionCallOutputContentItem::InputImage {
                        image_url: "https://example.test/result.png".to_string(),
                        detail: Some(ImageDetail::Low),
                    },
                    FunctionCallOutputContentItem::InputAudio {
                        audio_url: "https://example.test/result.wav".to_string(),
                    },
                ]),
                success: Some(true),
            },
        ),
        history_result(
            "call-1",
            vec![
                HistoryToolResultContent::Text("text".to_string()),
                HistoryToolResultContent::Image {
                    source: HistoryMediaSource::Uri("https://example.test/result.png".to_string()),
                    detail: Some(HistoryImageDetail::Low),
                },
                HistoryToolResultContent::Audio {
                    source: HistoryMediaSource::Uri("https://example.test/result.wav".to_string()),
                },
            ],
            Some(false),
        ),
    );
    for (success, is_error) in [
        (Some(true), Some(false)),
        (Some(false), Some(true)),
        (None, None),
    ] {
        canonical(
            custom_result("custom-result", text_output("done", success)),
            history_result(
                "custom-result",
                vec![HistoryToolResultContent::Text("done".to_string())],
                is_error,
            ),
        );
    }
}

#[test]
fn tool_result_encrypted_content_and_empty_call_ids_fall_back() {
    for (item, reason) in [
        (
            function_result(
                Some("call-1"),
                FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::EncryptedContent {
                        encrypted_content: "opaque".to_string(),
                    },
                ]),
            ),
            HistoryProjectionFallback::EncryptedToolResultContent,
        ),
        (
            function_result(Some(""), text_output("done", None)),
            HistoryProjectionFallback::MissingFunctionCallOutputCallId,
        ),
        (
            custom_result("", text_output("done", None)),
            HistoryProjectionFallback::MissingCustomToolCallOutputCallId,
        ),
    ] {
        fallback(item, reason);
    }
}

fn unsupported(kind: &str) -> ResponseItem {
    let value = match kind {
        "additional_tools" => r#"{"type":"additional_tools","role":"assistant","tools":[]}"#,
        "agent_message" => {
            r#"{"type":"agent_message","author":"agent-a","recipient":"agent-b","content":[]}"#
        }
        "local_shell" => {
            r#"{"type":"local_shell_call","call_id":"shell-1","status":"completed","action":{"type":"exec","command":["pwd"],"timeout_ms":null,"working_directory":null,"env":null,"user":null}}"#
        }
        "image_generation" => {
            r#"{"type":"image_generation_call","status":"completed","result":"image"}"#
        }
        "web_search" => r#"{"type":"web_search_call","status":"completed"}"#,
        "tool_search_output" => {
            r#"{"type":"tool_search_output","call_id":"search-1","status":"completed","execution":"client","tools":[{"type":"function"}]}"#
        }
        "compaction" => r#"{"type":"compaction","encrypted_content":"opaque"}"#,
        "compaction_trigger" => r#"{"type":"compaction_trigger"}"#,
        "context_compaction" => r#"{"type":"context_compaction"}"#,
        _ => panic!("unknown unsupported fixture: {kind}"),
    };
    serde_json::from_str(value).expect("response item fixture")
}

#[test]
fn unsupported_variant_classes_remain_compatibility_items() {
    use HistoryProjectionFallback::*;

    for (kind, reason) in [
        ("additional_tools", UnsupportedAdditionalTools),
        ("agent_message", UnsupportedAgentMessage),
        ("local_shell", UnsupportedLocalShell),
        ("image_generation", UnsupportedImageGeneration),
        ("web_search", UnsupportedWebSearch),
        ("tool_search_output", UnsupportedToolSearchOutput),
        ("compaction", UnsupportedCompaction),
        ("compaction_trigger", UnsupportedCompaction),
        ("context_compaction", UnsupportedCompaction),
    ] {
        fallback(unsupported(kind), reason);
    }
    fallback(
        ResponseItem::Other,
        HistoryProjectionFallback::UnsupportedOther,
    );
}
