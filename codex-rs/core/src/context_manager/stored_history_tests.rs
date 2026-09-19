use super::stored_history::StoredHistoryEntry;
use codex_history::CodexHarnessMetadata;
use codex_history::HistoryItem;
use codex_history::HistoryMessageRole;
use codex_history::HistoryProjectionFallback;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

fn message(role: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn canonical_message_role_drives_turn_boundary() {
    let entry = StoredHistoryEntry::new(ResponseItemEnvelope::new(message("user", "hello")));

    assert!(matches!(
        entry.canonical(),
        Some(HistoryItem::Message(message)) if message.role == HistoryMessageRole::User
    ));
    assert!(entry.is_user_turn_boundary());
    assert!(entry.fallback().is_none());
}

#[test]
fn encrypted_reasoning_retains_exact_fallback_source() {
    let source = ResponseItemEnvelope::with_metadata(
        ResponseItem::Reasoning {
            id: None,
            summary: vec![ReasoningItemReasoningSummary::SummaryText {
                text: "summary".to_string(),
            }],
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: "plain".to_string(),
            }]),
            encrypted_content: Some("opaque".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
        CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(2048),
        },
    );
    let entry = StoredHistoryEntry::new(source.clone());

    assert_eq!(entry.canonical(), None);
    assert_eq!(
        entry.fallback(),
        Some(HistoryProjectionFallback::EncryptedReasoning)
    );
    assert_eq!(entry.responses_compatibility(), &source);
    assert_eq!(entry.into_responses_compatibility(), source);
}

#[test]
fn unknown_role_retains_exact_fallback_source_after_replacement() {
    let unknown = ResponseItemEnvelope::new(message("future_role", "opaque"));
    let mut entry = StoredHistoryEntry::new(unknown.clone());
    assert_eq!(
        entry.fallback(),
        Some(HistoryProjectionFallback::UnknownMessageRole)
    );
    assert_eq!(entry.responses_compatibility(), &unknown);
    assert_eq!(entry.clone().into_responses_compatibility(), unknown);

    entry = StoredHistoryEntry::new(ResponseItemEnvelope::new(message("assistant", "known")));
    assert!(matches!(
        entry.canonical(),
        Some(HistoryItem::Message(message)) if message.role == HistoryMessageRole::Assistant
    ));
    assert_eq!(entry.fallback(), None);
    assert_eq!(
        entry.responses_compatibility().item,
        message("assistant", "known")
    );
}
