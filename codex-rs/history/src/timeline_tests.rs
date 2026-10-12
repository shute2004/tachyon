use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::json;
use tachyon_model::ModelContent;
use tachyon_model::ModelImageDetail;
use tachyon_model::ModelInputItem;
use tachyon_model::ModelMediaSource;
use tachyon_model::ModelMessage;
use tachyon_model::ModelMessageRole;
use tachyon_model::ModelToolCall;
use tachyon_model::ModelToolCallId;
use tachyon_model::ModelToolInput;

use crate::HistoryAppendKind;
use crate::HistoryReplacement;
use crate::HistoryTimeline;

#[test]
fn history_timeline_clone_and_default_do_not_require_item_traits() {
    struct NonCloneNonDefault;

    let timeline: HistoryTimeline<NonCloneNonDefault> = HistoryTimeline::default();
    let snapshot = timeline.clone();

    assert!(snapshot.items().is_empty());
    assert_eq!(snapshot.history_version(), 0);
    assert_eq!(snapshot.user_message_revision(), 0);
}

#[test]
fn history_timeline_tracks_append_replacement_and_snapshot_revisions() {
    let mut timeline = HistoryTimeline::default();
    timeline.record("assistant", HistoryAppendKind::Ordinary);
    timeline.record("user", HistoryAppendKind::UserInput);

    let before_compaction = timeline.clone();
    assert!(std::ptr::eq(
        timeline.items().as_ptr(),
        before_compaction.items().as_ptr()
    ));
    assert_eq!(timeline.history_version(), 0);
    assert_eq!(timeline.user_message_revision(), 1);

    timeline.replace(vec!["compacted"], HistoryReplacement::Compaction);
    assert_eq!(timeline.items(), &["compacted"][..]);
    assert_eq!(timeline.history_version(), 1);
    assert_eq!(timeline.user_message_revision(), 1);
    assert_eq!(before_compaction.items(), &["assistant", "user"][..]);
    assert_eq!(before_compaction.history_version(), 0);
    assert_eq!(before_compaction.user_message_revision(), 1);

    timeline.replace(vec!["reset"], HistoryReplacement::Reset);
    assert_eq!(timeline.items(), &["reset"][..]);
    assert_eq!(timeline.history_version(), 2);
    assert_eq!(timeline.user_message_revision(), 2);

    let before_mutation = timeline.clone();
    timeline.items_mut().push("normalized");
    assert!(!std::ptr::eq(
        timeline.items().as_ptr(),
        before_mutation.items().as_ptr()
    ));
    assert_eq!(before_mutation.items(), &["reset"][..]);
    assert_eq!(timeline.history_version(), 2);
    assert_eq!(timeline.user_message_revision(), 2);
}

#[test]
fn history_timeline_preserves_canonical_json_text_and_shared_media_values() {
    let image_bytes: Arc<[u8]> = Arc::from(vec![1_u8, 2, 3, 4]);
    let items = vec![
        ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId("json-call".to_string()),
            namespace: None,
            name: "json_tool".to_string(),
            input: ModelToolInput::Json(json!({"value": 7})),
        }),
        ModelInputItem::ToolCall(ModelToolCall {
            call_id: ModelToolCallId("text-call".to_string()),
            namespace: None,
            name: "text_tool".to_string(),
            input: ModelToolInput::Text("plain text input".to_string()),
        }),
        ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![ModelContent::Image {
                source: ModelMediaSource::Bytes {
                    media_type: "image/png".to_string(),
                    data: Arc::clone(&image_bytes),
                },
                detail: Some(ModelImageDetail::High),
            }],
        }),
    ];

    let mut timeline = HistoryTimeline::default();
    for item in items.iter().cloned() {
        timeline.record(item, HistoryAppendKind::Ordinary);
    }

    assert_eq!(timeline.items(), items.as_slice());
    let ModelInputItem::Message(message) = &timeline.items()[2] else {
        panic!("expected byte-backed message");
    };
    let [
        ModelContent::Image {
            source: ModelMediaSource::Bytes { data, .. },
            ..
        },
    ] = message.content.as_slice()
    else {
        panic!("expected byte-backed image content");
    };
    assert!(Arc::ptr_eq(&image_bytes, data));

    let collected = timeline.into_items();
    assert_eq!(collected, items);
}
