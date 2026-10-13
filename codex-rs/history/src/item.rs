//! Compatibility names for the provider-neutral semantic items in conversation history.
//!
//! These aliases preserve existing `codex_history::History*` paths while sharing the canonical
//! model request vocabulary. Persisted rollout records still use their existing compatibility
//! representation; this does not make durable history fully provider-neutral.

pub use tachyon_model::ModelContent as HistoryMessageContent;
pub use tachyon_model::ModelImageDetail as HistoryImageDetail;
pub use tachyon_model::ModelInputItem as HistoryItem;
pub use tachyon_model::ModelMediaSource as HistoryMediaSource;
pub use tachyon_model::ModelMessage as HistoryMessage;
pub use tachyon_model::ModelMessagePhase as HistoryMessagePhase;
pub use tachyon_model::ModelMessageRole as HistoryMessageRole;
pub use tachyon_model::ModelReasoning as HistoryReasoning;
pub use tachyon_model::ModelToolCall as HistoryToolCall;
pub use tachyon_model::ModelToolCallId as HistoryToolCallId;
pub use tachyon_model::ModelToolInput as HistoryToolInput;
pub use tachyon_model::ModelToolResult as HistoryToolResult;
pub use tachyon_model::ModelToolResultContent as HistoryToolResultContent;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use serde_json::Value;

    use super::*;

    #[test]
    fn history_names_are_the_canonical_model_item_types() {
        let data: Arc<[u8]> = Arc::from([0_u8, 1, 2, 3]);
        let model_item: tachyon_model::ModelInputItem =
            tachyon_model::ModelInputItem::Message(tachyon_model::ModelMessage {
                role: tachyon_model::ModelMessageRole::User,
                phase: None,
                content: vec![tachyon_model::ModelContent::Image {
                    source: tachyon_model::ModelMediaSource::Bytes {
                        media_type: "image/png".to_string(),
                        data: Arc::clone(&data),
                    },
                    detail: Some(tachyon_model::ModelImageDetail::High),
                }],
            });
        let history_item: HistoryItem = model_item.clone();
        let _: &tachyon_model::ModelInputItem = &history_item;
        assert_eq!(history_item, model_item);
        let HistoryItem::Message(message) = history_item else {
            panic!("expected the aliased canonical message type");
        };
        let [
            HistoryMessageContent::Image {
                source:
                    HistoryMediaSource::Bytes {
                        data: cloned_data, ..
                    },
                ..
            },
        ] = message.content.as_slice()
        else {
            panic!("expected byte-backed image content");
        };
        assert!(Arc::ptr_eq(&data, cloned_data));

        let model_result =
            tachyon_model::ModelInputItem::ToolResult(tachyon_model::ModelToolResult {
                call_id: tachyon_model::ModelToolCallId("discovery".to_string()),
                content: vec![
                    tachyon_model::ModelToolResultContent::Json(Value::Null),
                    tachyon_model::ModelToolResultContent::DiscoveredTools(Vec::new()),
                ],
                is_error: Some(false),
            });
        let history_result: HistoryItem = model_result.clone();
        assert_eq!(history_result, model_result);
        let HistoryItem::ToolResult(result) = history_result else {
            panic!("expected the aliased canonical tool-result type");
        };
        assert!(matches!(
            result.content.as_slice(),
            [
                HistoryToolResultContent::Json(Value::Null),
                HistoryToolResultContent::DiscoveredTools(tools)
            ] if tools.is_empty()
        ));
    }

    #[test]
    fn top_level_history_categories_are_distinct() {
        let items = [
            HistoryItem::Message(HistoryMessage {
                role: HistoryMessageRole::User,
                phase: None,
                content: vec![HistoryMessageContent::Text("hello".to_string())],
            }),
            HistoryItem::Reasoning(HistoryReasoning {
                summary: vec!["summary".to_string()],
                content: vec!["plain".to_string()],
            }),
            HistoryItem::ToolCall(HistoryToolCall {
                call_id: HistoryToolCallId("call-1".to_string()),
                namespace: Some("shell".to_string()),
                name: "run".to_string(),
                input: HistoryToolInput::Text("pwd".to_string()),
            }),
            HistoryItem::ToolResult(HistoryToolResult {
                call_id: HistoryToolCallId("call-1".to_string()),
                content: vec![HistoryToolResultContent::Text("/tmp".to_string())],
                is_error: Some(false),
            }),
        ];

        assert!(matches!(items[0], HistoryItem::Message(_)));
        assert!(matches!(items[1], HistoryItem::Reasoning(_)));
        assert!(matches!(items[2], HistoryItem::ToolCall(_)));
        assert!(matches!(items[3], HistoryItem::ToolResult(_)));
    }

    #[test]
    fn tool_input_keeps_json_and_text_distinct() {
        let json = HistoryToolInput::Json(serde_json::json!({"command": "pwd"}));
        let text = HistoryToolInput::Text("{\"command\":\"pwd\"}".to_string());

        assert_ne!(json, text);
        assert!(matches!(json, HistoryToolInput::Json(Value::Object(_))));
        assert!(matches!(text, HistoryToolInput::Text(_)));
    }

    #[test]
    fn tool_result_preserves_tri_state_error_status() {
        let result = |is_error| HistoryToolResult {
            call_id: HistoryToolCallId("call-1".to_string()),
            content: Vec::new(),
            is_error,
        };

        assert_eq!(result(Some(true)).is_error, Some(true));
        assert_eq!(result(Some(false)).is_error, Some(false));
        assert_eq!(result(None).is_error, None);
    }

    #[test]
    fn media_supports_uri_and_bytes_sources() {
        let uri = HistoryMediaSource::Uri("https://example.test/image.png".to_string());
        let bytes = HistoryMediaSource::Bytes {
            media_type: "image/png".to_string(),
            data: Arc::from([0_u8, 1, 2, 3]),
        };

        let image = HistoryMessageContent::Image {
            source: uri.clone(),
            detail: Some(HistoryImageDetail::High),
        };
        let audio = HistoryMessageContent::Audio {
            source: bytes.clone(),
        };

        assert!(matches!(uri, HistoryMediaSource::Uri(_)));
        assert!(matches!(bytes, HistoryMediaSource::Bytes { .. }));
        assert!(matches!(image, HistoryMessageContent::Image { .. }));
        assert!(matches!(audio, HistoryMessageContent::Audio { .. }));
    }
}
