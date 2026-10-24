use std::mem::size_of;

use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::ExecutedToolCall;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use serde_json::json;

use super::CodexHarnessMetadata;
use super::ResponseItemEnvelope;
use super::RetainedHistory;
use super::RetainedHistoryAppendOutcome;
use super::RetainedHistoryBudget;
use super::RetainedHistoryClass;
use super::RetainedHistoryClassBudget;
use super::RetainedHistoryRejection;
use super::response_retention::*;

fn envelope(item: ResponseItem) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(item)
}

fn message(role: &str, content: Vec<ContentItem>) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn admitted_charge(envelope: &ResponseItemEnvelope) -> usize {
    prepare_response_item_retention(envelope, usize::MAX)
        .expect("unbounded budget admits known fixture")
        .charged_bytes()
}

fn assert_serialized_charge_is_covered(
    prepared: &PreparedResponseItemRetention<'_>,
    budget: usize,
) {
    let item_bytes = serde_json::to_vec(&prepared.envelope().item)
        .expect("serialize prepared item")
        .len();
    let metadata_bytes = prepared
        .envelope()
        .metadata
        .as_ref()
        .map(|metadata| {
            serde_json::to_vec(metadata)
                .expect("serialize sidecar metadata")
                .len()
        })
        .unwrap_or(0);
    let serialized_charge = size_of::<ResponseItemEnvelope>() + item_bytes + metadata_bytes;
    assert!(
        serialized_charge <= prepared.charged_bytes(),
        "serialized prepared item charge {serialized_charge} exceeds reported charge {}",
        prepared.charged_bytes()
    );
    assert!(
        prepared.charged_bytes() <= budget,
        "reported charge {} exceeds supplied budget {budget}",
        prepared.charged_bytes()
    );
}

fn minimum_media_fallback_budget(envelope: &ResponseItemEnvelope) -> usize {
    let full_charge = admitted_charge(envelope);
    let mut low = 0;
    let mut high = full_charge - 1;
    assert!(
        prepare_response_item_retention(envelope, high)
            .is_ok_and(|prepared| prepared.omitted_media()),
        "fixture must have a fitting text fallback below its full-media charge"
    );
    while low < high {
        let middle = low + (high - low) / 2;
        if prepare_response_item_retention(envelope, middle)
            .is_ok_and(|prepared| prepared.omitted_media())
        {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

#[test]
fn exact_limit_is_admitted_and_one_byte_below_is_rejected() {
    let source = envelope(message(
        "assistant",
        vec![ContentItem::OutputText {
            text: "line\n猫 \"quoted\"".to_string(),
        }],
    ));
    let exact = admitted_charge(&source);
    let expected = size_of::<ResponseItemEnvelope>()
        + serde_json::to_vec(&source.item)
            .expect("serialize item for expected charge")
            .len();
    assert_eq!(exact, expected);
    let prepared = prepare_response_item_retention(&source, exact).expect("exact limit fits");
    assert_eq!(prepared.charged_bytes(), exact);
    assert!(!prepared.omitted_media());
    assert!(std::ptr::eq(prepared.envelope(), &source));
    assert_eq!(
        prepare_response_item_retention(&source, exact - 1).err(),
        Some(ResponseItemRetentionRejection::OverBudget)
    );
    assert_eq!(
        prepare_response_item_retention(&source, 0).err(),
        Some(ResponseItemRetentionRejection::OverBudget)
    );
}

#[test]
fn sidecar_and_analytics_metadata_are_included_in_the_charge() {
    let item = message(
        "assistant",
        vec![ContentItem::OutputText { text: "ok".into() }],
    );
    let no_metadata = envelope(item.clone());
    let mut internal = InternalChatMessageMetadataPassthrough::default();
    internal.turn_id = Some("turn".repeat(100));
    internal.content_item_kinds = Some(vec![ContentItemKind("answer".into())]);
    internal.executed_tool_calls = Some(vec![ExecutedToolCall::new(
        "analytics-event".into(),
        json!({"payload": "x".repeat(512)}),
    )]);
    let ResponseItem::Message {
        id,
        role,
        content,
        phase,
        ..
    } = item
    else {
        unreachable!()
    };
    let with_metadata = ResponseItem::Message {
        id,
        role,
        content,
        phase,
        internal_chat_message_metadata_passthrough: Some(internal),
    };
    let with_metadata = ResponseItemEnvelope::with_metadata(
        with_metadata,
        CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(99),
        },
    );
    let base_charge = admitted_charge(&no_metadata);
    let full_charge = admitted_charge(&with_metadata);
    assert!(full_charge > base_charge);
    assert_eq!(
        prepare_response_item_retention(&with_metadata, base_charge).err(),
        Some(ResponseItemRetentionRejection::OverBudget)
    );
}

#[test]
fn reasoning_content_is_charged_even_when_the_main_serializer_skips_it() {
    for content in [
        vec![ReasoningItemContent::Text {
            text: "skipped by main serializer".into(),
        }],
        vec![
            ReasoningItemContent::Text {
                text: "text".into(),
            },
            ReasoningItemContent::ReasoningText {
                text: "included by main serializer".into(),
            },
        ],
    ] {
        let item = ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: Some(content.clone()),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        };
        let source = envelope(item.clone());
        let main_json = serde_json::to_value(&item).expect("serialize reasoning item");
        let item_bytes = serde_json::to_vec(&item)
            .expect("serialize reasoning item")
            .len();
        let content_bytes = serde_json::to_vec(&content)
            .expect("serialize reasoning content")
            .len();
        let expected = size_of::<ResponseItemEnvelope>() + item_bytes + content_bytes;
        if content
            .iter()
            .all(|part| matches!(part, ReasoningItemContent::Text { .. }))
        {
            assert!(main_json.get("content").is_none());
        } else {
            assert!(main_json.get("content").is_some());
        }
        assert_eq!(admitted_charge(&source), expected);
    }
}

#[test]
fn compaction_controls_and_annotated_summary_messages_are_rejected() {
    let controls = [
        ResponseItem::Compaction {
            id: None,
            encrypted_content: String::new(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::CompactionTrigger {},
        ResponseItem::ContextCompaction {
            id: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    for item in controls {
        assert_eq!(
            prepare_response_item_retention(&envelope(item), usize::MAX).err(),
            Some(ResponseItemRetentionRejection::CompactionControl)
        );
    }

    let annotated = ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![
            ContentItem::InputText {
                text: "summary".into(),
            },
            ContentItem::InputImage {
                image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                detail: None,
            },
        ],
        phase: None,
        internal_chat_message_metadata_passthrough: Some(InternalChatMessageMetadataPassthrough {
            content_item_kinds: Some(vec![ContentItemKind("compaction.summary".into())]),
            ..InternalChatMessageMetadataPassthrough::default()
        }),
    };
    assert_eq!(
        prepare_response_item_retention(&envelope(annotated), 1).err(),
        Some(ResponseItemRetentionRejection::CompactionControl)
    );
}

#[test]
fn unknown_wire_variant_is_rejected_after_its_payload_was_lost() {
    let ghost: ResponseItem =
        serde_json::from_str(r#"{"type":"future_ghost","payload":"unrecoverable original data"}"#)
            .expect("unknown tagged item maps to ResponseItem::Other");
    assert!(matches!(ghost, ResponseItem::Other));
    assert_eq!(
        prepare_response_item_retention(&envelope(ghost), usize::MAX).err(),
        Some(ResponseItemRetentionRejection::UnsupportedItem)
    );
}

#[test]
fn fitting_full_media_borrows_the_original_envelope_unchanged() {
    let source = envelope(message(
        "user",
        vec![
            ContentItem::InputText {
                text: "caption".into(),
            },
            ContentItem::InputImage {
                image_url: "data:image/png;base64,small".into(),
                detail: None,
            },
        ],
    ));
    let original = source.clone();
    let prepared = prepare_response_item_retention(&source, usize::MAX).expect("fits");
    assert!(std::ptr::eq(prepared.envelope(), &source));
    assert!(!prepared.omitted_media());
    assert_eq!(source, original);
}

#[test]
fn fallback_preserves_text_order_fields_sidecar_and_aligned_kinds() {
    let source = ResponseItemEnvelope::with_metadata(
        ResponseItem::Message {
            id: Some(ResponseItemId::from_server("msg_fallback".into())),
            role: "user".into(),
            content: vec![
                ContentItem::InputText {
                    text: "before".into(),
                },
                ContentItem::InputImage {
                    image_url: format!("data:image/png;base64,{}", "i".repeat(4096)),
                    detail: None,
                },
                ContentItem::InputAudio {
                    audio_url: format!("data:audio/wav;base64,{}", "a".repeat(2048)),
                },
                ContentItem::InputText {
                    text: String::new(),
                },
                ContentItem::OutputText {
                    text: "middle".into(),
                },
                ContentItem::OutputText {
                    text: String::new(),
                },
                ContentItem::InputText {
                    text: "after".into(),
                },
            ],
            phase: Some(MessagePhase::Commentary),
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("turn_42".into()),
                    create_time: Some(serde_json::Number::from(123_i64)),
                    content_item_kinds: Some(
                        [
                            "before-kind",
                            "image-kind",
                            "audio-kind",
                            "empty-input-kind",
                            "middle-kind",
                            "empty-output-kind",
                            "after-kind",
                        ]
                        .into_iter()
                        .map(|kind| ContentItemKind(kind.into()))
                        .collect(),
                    ),
                    executed_tool_calls: Some(vec![ExecutedToolCall::new(
                        "saved-call".into(),
                        json!({"small": true}),
                    )]),
                },
            ),
        },
        CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(300),
        },
    );
    let original = source.clone();
    let exact_fallback = minimum_media_fallback_budget(&source);
    let prepared = prepare_response_item_retention(&source, exact_fallback).expect("fallback fits");
    assert_serialized_charge_is_covered(&prepared, exact_fallback);
    assert!(prepared.omitted_media());
    assert_eq!(prepared.charged_bytes(), exact_fallback);
    let ResponseItem::Message {
        id,
        role,
        content,
        phase,
        internal_chat_message_metadata_passthrough: Some(metadata),
    } = &prepared.envelope().item
    else {
        panic!("fallback retains a message and its metadata");
    };
    assert_eq!(id.as_ref().unwrap().as_str(), "msg_fallback");
    assert_eq!(role, "user");
    assert_eq!(
        content,
        &[
            ContentItem::InputText {
                text: "before".into()
            },
            ContentItem::InputText {
                text: String::new()
            },
            ContentItem::OutputText {
                text: "middle".into()
            },
            ContentItem::OutputText {
                text: String::new()
            },
            ContentItem::InputText {
                text: "after".into()
            },
        ]
    );
    assert_eq!(phase, &Some(MessagePhase::Commentary));
    assert_eq!(metadata.turn_id.as_deref(), Some("turn_42"));
    assert_eq!(
        metadata.create_time,
        Some(serde_json::Number::from(123_i64))
    );
    assert_eq!(
        metadata.executed_tool_calls.as_ref().unwrap(),
        &[ExecutedToolCall::new(
            "saved-call".into(),
            json!({"small": true})
        )]
    );
    assert_eq!(
        metadata
            .content_item_kinds
            .as_ref()
            .unwrap()
            .iter()
            .map(|kind| kind.0.as_str())
            .collect::<Vec<_>>(),
        [
            "before-kind",
            "empty-input-kind",
            "middle-kind",
            "empty-output-kind",
            "after-kind"
        ]
    );
    assert_eq!(
        prepared.envelope().metadata,
        Some(CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(300),
        })
    );
    assert_eq!(source, original);
    assert_eq!(
        prepare_response_item_retention(&source, exact_fallback - 1).err(),
        Some(ResponseItemRetentionRejection::OverBudget)
    );
}

#[test]
fn fallback_clears_misaligned_content_kinds_instead_of_relabeling_text() {
    let source = envelope(ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![
            ContentItem::InputText {
                text: "keep".into(),
            },
            ContentItem::InputImage {
                image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                detail: None,
            },
            ContentItem::OutputText {
                text: "also keep".into(),
            },
        ],
        phase: None,
        internal_chat_message_metadata_passthrough: Some(InternalChatMessageMetadataPassthrough {
            content_item_kinds: Some(vec![
                ContentItemKind("misaligned-a".into()),
                ContentItemKind("misaligned-b".into()),
            ]),
            ..InternalChatMessageMetadataPassthrough::default()
        }),
    });
    let budget = minimum_media_fallback_budget(&source);
    let prepared = prepare_response_item_retention(&source, budget).expect("fallback fits");
    assert_serialized_charge_is_covered(&prepared, budget);
    let ResponseItem::Message {
        internal_chat_message_metadata_passthrough: Some(metadata),
        ..
    } = &prepared.envelope().item
    else {
        panic!("fallback retains message metadata");
    };
    assert_eq!(metadata.content_item_kinds, None);
}

#[test]
fn unsupported_fallback_shapes_and_oversized_text_fail_closed() {
    let max_bytes = 512;
    let cases = [
        message(
            "assistant",
            vec![
                ContentItem::OutputText {
                    text: "answer".into(),
                },
                ContentItem::InputImage {
                    image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                    detail: None,
                },
            ],
        ),
        message(
            "user",
            vec![ContentItem::InputImage {
                image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                detail: None,
            }],
        ),
        message(
            "user",
            vec![ContentItem::InputAudio {
                audio_url: format!("data:audio/wav;base64,{}", "x".repeat(1024)),
            }],
        ),
        message(
            "user",
            vec![
                ContentItem::InputText {
                    text: "z".repeat(1024),
                },
                ContentItem::InputImage {
                    image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                    detail: None,
                },
            ],
        ),
        message(
            "user",
            vec![
                ContentItem::InputText {
                    text: String::new(),
                },
                ContentItem::InputImage {
                    image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                    detail: None,
                },
            ],
        ),
        message(
            "user",
            vec![
                ContentItem::OutputText {
                    text: String::new(),
                },
                ContentItem::InputImage {
                    image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                    detail: None,
                },
            ],
        ),
        {
            let mut metadata = InternalChatMessageMetadataPassthrough::default();
            metadata.turn_id = Some("large".repeat(200));
            ResponseItem::Message {
                id: None,
                role: "user".into(),
                content: vec![
                    ContentItem::InputText {
                        text: "small".into(),
                    },
                    ContentItem::InputImage {
                        image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                        detail: None,
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: Some(metadata),
            }
        },
    ];
    for item in cases {
        assert_eq!(
            prepare_response_item_retention(&envelope(item), max_bytes).err(),
            Some(ResponseItemRetentionRejection::OverBudget)
        );
    }
}

#[test]
fn whitespace_is_nonempty_text_for_media_fallback() {
    let source = envelope(message(
        "user",
        vec![
            ContentItem::InputText { text: " \n".into() },
            ContentItem::InputImage {
                image_url: format!("data:image/png;base64,{}", "x".repeat(1024)),
                detail: None,
            },
        ],
    ));
    let budget = minimum_media_fallback_budget(&source);
    let prepared = prepare_response_item_retention(&source, budget).expect("whitespace text fits");
    assert!(prepared.omitted_media());
    let ResponseItem::Message { content, .. } = &prepared.envelope().item else {
        panic!("fallback retains a message");
    };
    assert_eq!(content, &[ContentItem::InputText { text: " \n".into() }]);
}

#[test]
fn prepared_fallback_charge_is_consumed_by_the_generic_retention_store() {
    let source = envelope(message(
        "user",
        vec![
            ContentItem::InputText {
                text: "keep".into(),
            },
            ContentItem::InputImage {
                image_url: format!("data:image/png;base64,{}", "x".repeat(2048)),
                detail: None,
            },
        ],
    ));
    let budget_bytes = minimum_media_fallback_budget(&source);
    let prepared = prepare_response_item_retention(&source, budget_bytes).expect("fallback fits");
    let (envelope, charged_bytes, omitted_media) = prepared.into_parts();
    assert!(
        omitted_media,
        "omission status travels with the consumed payload"
    );

    let class_budget = RetainedHistoryClassBudget::new(1, charged_bytes);
    let budget = RetainedHistoryBudget::new(class_budget, class_budget);
    let mut history = RetainedHistory::new(budget);
    let prepared_envelope = envelope.into_owned();
    assert_eq!(
        history.append(
            RetainedHistoryClass::Ordinary,
            charged_bytes,
            &prepared_envelope,
        ),
        RetainedHistoryAppendOutcome::Appended {
            evicted: Default::default()
        }
    );
    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.entries()[0].charged_bytes(), charged_bytes);

    let too_small = RetainedHistoryClassBudget::new(1, charged_bytes - 1);
    let mut rejecting_history =
        RetainedHistory::new(RetainedHistoryBudget::new(too_small, too_small));
    assert_eq!(
        rejecting_history.append(
            RetainedHistoryClass::Ordinary,
            charged_bytes,
            &prepared_envelope,
        ),
        RetainedHistoryAppendOutcome::Rejected(RetainedHistoryRejection::EntryExceedsByteLimit {
            class: RetainedHistoryClass::Ordinary,
            charged_bytes,
            max_charged_bytes: charged_bytes - 1,
        })
    );
    assert!(rejecting_history.entries().is_empty());
}
