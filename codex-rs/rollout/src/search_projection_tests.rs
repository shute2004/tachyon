use codex_history::CodexHarnessMetadata;
use codex_history::HistoryItemProjection;
use codex_history::HistoryProjectionFallback;
use codex_history::project_response_item;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ResponseItem;
use serde_json::json;

use super::conversation_text_from_item;
use crate::ResponseItemEnvelope;
use crate::RolloutItem;
use crate::RolloutLine;

fn message(role: &str, content: Vec<ContentItem>) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn response_item(item: ResponseItem) -> RolloutItem {
    RolloutItem::ResponseItem(ResponseItemEnvelope::new(item))
}

#[test]
fn canonical_user_and_assistant_messages_use_projected_text() {
    for (role, expected) in [("user", "user text"), ("assistant", "assistant text")] {
        let item = response_item(message(
            role,
            vec![ContentItem::InputText {
                text: expected.to_string(),
            }],
        ));

        assert_eq!(
            conversation_text_from_item(&item),
            Some(expected.to_string())
        );
    }
}

#[test]
fn canonical_message_text_ignores_images_and_audio_in_source_order() {
    let item = response_item(message(
        "assistant",
        vec![
            ContentItem::InputText {
                text: "first".to_string(),
            },
            ContentItem::InputImage {
                image_url: "https://example.test/image".to_string(),
                detail: Some(ImageDetail::High),
            },
            ContentItem::InputAudio {
                audio_url: "data:audio/wav;base64,AA==".to_string(),
            },
            ContentItem::OutputText {
                text: "last".to_string(),
            },
        ],
    ));

    assert_eq!(
        conversation_text_from_item(&item),
        Some("first last".to_string())
    );
}

#[test]
fn metadata_bearing_envelope_is_preserved_during_projection() {
    let envelope = ResponseItemEnvelope::with_metadata(
        message(
            "user",
            vec![ContentItem::InputText {
                text: "metadata stays attached".to_string(),
            }],
        ),
        CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(4096),
        },
    );
    let item = RolloutItem::ResponseItem(envelope.clone());
    let line = RolloutLine {
        timestamp: "2026-09-01T00:00:00Z".to_string(),
        ordinal: Some(7),
        item: item.clone(),
    };
    let encoded_before = serde_json::to_vec(&line).expect("rollout line should serialize");

    let projection = project_response_item(envelope.clone());
    match projection {
        HistoryItemProjection::Canonical { compatibility, .. } => {
            assert_eq!(compatibility, envelope);
        }
        HistoryItemProjection::Fallback { .. } => panic!("expected canonical message"),
    }
    assert_eq!(
        conversation_text_from_item(&item),
        Some("metadata stays attached".to_string())
    );

    let encoded_after = serde_json::to_vec(&line).expect("rollout line should serialize");
    assert_eq!(encoded_after, encoded_before);
}

#[test]
fn unknown_role_falls_back_to_legacy_extraction_without_becoming_searchable() {
    let envelope = ResponseItemEnvelope::new(message(
        "future_role",
        vec![ContentItem::InputText {
            text: "opaque future-role text".to_string(),
        }],
    ));
    assert!(matches!(
        project_response_item(envelope.clone()),
        HistoryItemProjection::Fallback {
            reason: HistoryProjectionFallback::UnknownMessageRole,
            ..
        }
    ));

    let item = RolloutItem::ResponseItem(envelope);
    assert_eq!(conversation_text_from_item(&item), None);
}

#[test]
fn projected_non_message_and_unsupported_items_are_not_searchable() {
    let projected_tool_search = response_item(ResponseItem::ToolSearchCall {
        id: None,
        call_id: Some("tool-search-1".to_string()),
        status: Some("completed".to_string()),
        execution: "client".to_string(),
        arguments: json!({"query": "private search phrase"}),
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(conversation_text_from_item(&projected_tool_search), None);

    let unsupported_agent_message = response_item(ResponseItem::AgentMessage {
        id: None,
        author: "agent".to_string(),
        recipient: "user".to_string(),
        content: vec![AgentMessageInputContent::InputText {
            text: "unsupported agent phrase".to_string(),
        }],
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(
        conversation_text_from_item(&unsupported_agent_message),
        None
    );
}
