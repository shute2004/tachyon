use std::borrow::Cow;
use std::num::NonZeroU64;

use crate::CodexHarnessMetadata;
use crate::HistoryItem;
use crate::InputAssociation;
use crate::InputIdentity;
use crate::InputSource;
use crate::ResponseItemEnvelope;
use crate::RolloutItem;
use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

use super::ResponsesHistoryRecord;
use super::StoredPayload;

fn metadata() -> CodexHarnessMetadata {
    CodexHarnessMetadata {
        client_authored: true,
        reinjectable_context: true,
        fallback_token_limit_override: Some(19),
        input_association: Some(InputAssociation {
            identity: InputIdentity {
                thread_id: ThreadId::new(),
                incarnation: crate::InputStreamIncarnation::new(),
                sequence: NonZeroU64::new(4).expect("non-zero sequence"),
            },
            source: InputSource::Synthetic,
        }),
    }
}

fn passthrough() -> Option<InternalChatMessageMetadataPassthrough> {
    Some(InternalChatMessageMetadataPassthrough {
        turn_id: Some("turn-42".to_string()),
        create_time: Some(serde_json::Number::from_f64(123.5).expect("finite timestamp")),
        content_item_kinds: Some(vec![ContentItemKind("fixture-kind".to_string())]),
        executed_tool_calls: None,
    })
}

fn assert_roundtrip(item: ResponseItem, canonical: bool) {
    let envelope = ResponseItemEnvelope {
        item,
        metadata: Some(metadata()),
    };
    let serialized = serde_json::to_vec(&RolloutItem::ResponseItem(envelope.clone()))
        .expect("source rollout item serializes");
    let stored = ResponsesHistoryRecord::from_response_envelope(envelope.clone());

    assert_eq!(
        matches!(&stored.payload, StoredPayload::Canonical { .. }),
        canonical
    );
    assert_eq!(stored.metadata.as_ref(), envelope.metadata.as_ref());
    match (canonical, stored.response_item()) {
        (true, Cow::Owned(_)) | (false, Cow::Borrowed(_)) => {}
        (true, Cow::Borrowed(_)) => panic!("canonical response item is reconstructed"),
        (false, Cow::Owned(_)) => panic!("legacy response item remains borrowed"),
    }

    let projected = stored.response_envelope();
    assert_eq!(projected, envelope);
    assert_eq!(
        serde_json::to_vec(&RolloutItem::ResponseItem(projected))
            .expect("projected rollout item serializes"),
        serialized
    );
    assert_eq!(stored.into_response_envelope(), envelope);
}

#[test]
fn canonical_message_keeps_text_tags_media_uris_identity_and_metadata() {
    let image_uri = "data:image/png;base64,AAECAw==".to_string();
    let audio_uri = "https://example.test/audio.wav".to_string();
    let item = ResponseItem::Message {
        id: Some(ResponseItemId::from_server("msg_wire-id".to_string())),
        role: "assistant".to_string(),
        content: vec![
            ContentItem::InputText {
                text: "input-shaped text".to_string(),
            },
            ContentItem::InputImage {
                image_url: image_uri,
                detail: Some(ImageDetail::Original),
            },
            ContentItem::OutputText {
                text: "output-shaped text".to_string(),
            },
            ContentItem::InputAudio {
                audio_url: audio_uri,
            },
        ],
        phase: Some(MessagePhase::FinalAnswer),
        internal_chat_message_metadata_passthrough: passthrough(),
    };
    let fixture_metadata = metadata();
    let envelope = ResponseItemEnvelope {
        item: item.clone(),
        metadata: Some(fixture_metadata.clone()),
    };
    let stored = ResponsesHistoryRecord::from_response_envelope(envelope.clone());

    let StoredPayload::Canonical {
        item: HistoryItem::Message(message),
        ..
    } = &stored.payload
    else {
        panic!("known-role message should be represented canonically");
    };
    assert_eq!(message.content.len(), 4);
    assert_eq!(message.phase, Some(crate::HistoryMessagePhase::Final));
    assert_eq!(stored.response_item(), Cow::Owned(item.clone()));
    assert_eq!(stored.response_envelope().item, item);
    assert!(matches!(
        stored.response_item().as_ref(),
        ResponseItem::Message { content, .. }
            if matches!(content.as_slice(), [
                ContentItem::InputText { .. },
                ContentItem::InputImage { image_url, .. },
                ContentItem::OutputText { .. },
                ContentItem::InputAudio { audio_url },
            ] if image_url == "data:image/png;base64,AAECAw==" && audio_url == "https://example.test/audio.wav")
    ));
    assert_eq!(stored.metadata.as_ref(), Some(&fixture_metadata));
    assert_eq!(stored.response_envelope(), envelope);
}

#[test]
fn canonical_message_phase_uses_shared_history_semantics() {
    for (phase, expected) in [
        (
            Some(MessagePhase::Commentary),
            Some(crate::HistoryMessagePhase::Commentary),
        ),
        (
            Some(MessagePhase::FinalAnswer),
            Some(crate::HistoryMessagePhase::Final),
        ),
        (None, None),
    ] {
        let item = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "phase-aware text".to_string(),
            }],
            phase,
            internal_chat_message_metadata_passthrough: None,
        };
        let stored = ResponsesHistoryRecord::from_response_envelope(ResponseItemEnvelope {
            item: item.clone(),
            metadata: None,
        });

        let StoredPayload::Canonical {
            item: HistoryItem::Message(message),
            ..
        } = &stored.payload
        else {
            panic!("known-role message should be represented canonically");
        };
        assert_eq!(message.phase, expected);
        assert_eq!(stored.response_item().as_ref(), &item);
    }
}

#[test]
fn canonical_reasoning_keeps_content_presence_and_content_tags() {
    let cases = [
        Some(vec![
            ReasoningItemContent::ReasoningText {
                text: "private thought".to_string(),
            },
            ReasoningItemContent::Text {
                text: "plain continuation".to_string(),
            },
        ]),
        Some(Vec::new()),
        None,
    ];

    for content in cases {
        let item = ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server("rs_wire-id".to_string())),
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "summary".to_string(),
            }],
            content,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: passthrough(),
        };
        assert_roundtrip(item, /*canonical*/ true);
    }
}

#[test]
fn canonical_calls_and_outputs_keep_response_only_shape() {
    let cases = vec![
        ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server("fc_wire-id".to_string())),
            name: "read_file".to_string(),
            namespace: Some("filesystem".to_string()),
            arguments: r#"{"path":"a.txt"}"#.to_string(),
            encrypted_function_args: None,
            call_id: "call-1".to_string(),
            internal_chat_message_metadata_passthrough: passthrough(),
        },
        ResponseItem::CustomToolCall {
            id: Some(ResponseItemId::from_server("ctc_wire-id".to_string())),
            status: Some("incomplete".to_string()),
            call_id: "call-2".to_string(),
            name: "shell".to_string(),
            namespace: Some("local".to_string()),
            input: "pwd".to_string(),
            internal_chat_message_metadata_passthrough: passthrough(),
        },
        ResponseItem::FunctionCallOutput {
            id: Some(ResponseItemId::from_server("fco_wire-id".to_string())),
            call_id: Some("call-3".to_string()),
            name: Some("read_file".to_string()),
            namespace: Some("filesystem".to_string()),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "result".to_string(),
                    },
                    FunctionCallOutputContentItem::InputImage {
                        image_url: "data:image/png;base64,BAUG".to_string(),
                        detail: Some(ImageDetail::Low),
                    },
                    FunctionCallOutputContentItem::InputAudio {
                        audio_url: "https://example.test/result.wav".to_string(),
                    },
                ]),
                success: Some(true),
            },
            internal_chat_message_metadata_passthrough: passthrough(),
        },
        ResponseItem::CustomToolCallOutput {
            id: Some(ResponseItemId::from_server("ctco_wire-id".to_string())),
            call_id: "call-4".to_string(),
            name: Some("shell".to_string()),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text("command failed".to_string()),
                success: None,
            },
            internal_chat_message_metadata_passthrough: passthrough(),
        },
    ];

    for item in cases {
        assert_roundtrip(item, /*canonical*/ true);
    }

    for success in [None, Some(false), Some(true)] {
        assert_roundtrip(
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: Some("call-status".to_string()),
                name: None,
                namespace: None,
                output: FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text(String::new()),
                    success,
                },
                internal_chat_message_metadata_passthrough: None,
            },
            /*canonical*/ true,
        );
    }
}

#[test]
fn lossy_or_host_specific_items_stay_exact_legacy_records() {
    let cases = vec![
        ResponseItem::Message {
            id: None,
            role: "future_role".to_string(),
            content: vec![ContentItem::InputText {
                text: "unchanged".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "tool".to_string(),
            namespace: None,
            arguments: " {\"x\":1}".to_string(),
            encrypted_function_args: None,
            call_id: "call-space".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "tool".to_string(),
            namespace: None,
            arguments: "{\"x\":}".to_string(),
            encrypted_function_args: None,
            call_id: "call-invalid-json".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "tool".to_string(),
            namespace: None,
            arguments: "{\"x\":1}".to_string(),
            encrypted_function_args: Some(vec!["opaque".to_string()]),
            call_id: "call-encrypted".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: None,
            encrypted_content: Some("opaque reasoning".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::ToolSearchCall {
            id: None,
            call_id: Some("search-1".to_string()),
            status: Some("completed".to_string()),
            execution: "client".to_string(),
            arguments: serde_json::json!({"query": "tools"}),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::AdditionalTools {
            id: None,
            role: "assistant".to_string(),
            tools: vec![serde_json::json!({"name": "extension"})],
        },
        ResponseItem::CompactionTrigger {},
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-encrypted-result".to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::EncryptedContent {
                        encrypted_content: "opaque output".to_string(),
                    },
                ]),
                success: Some(false),
            },
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    for item in cases {
        assert_roundtrip(item, /*canonical*/ false);
    }
}
