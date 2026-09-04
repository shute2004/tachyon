//! Kernel-owned vocabulary for the semantic items that make up conversation history.
//!
//! These types describe durable harness meaning only. They intentionally do not carry wire
//! serialization derives: the existing rollout representation remains the compatibility format
//! until a later migration slice can project to these values without losing information.

use std::sync::Arc;

use serde_json::Value;

/// One semantic item in durable conversation history.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryItem {
    Message(HistoryMessage),
    Reasoning(HistoryReasoning),
    ToolCall(HistoryToolCall),
    ToolResult(HistoryToolResult),
}

/// A role-bearing message retained in conversation history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryMessage {
    pub role: HistoryMessageRole,
    /// The assistant lifecycle phase, when the source history distinguishes interim narration
    /// from the terminal answer.
    pub phase: Option<HistoryMessagePhase>,
    pub content: Vec<HistoryMessageContent>,
}

/// Stable semantic roles for history messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMessageRole {
    System,
    Developer,
    User,
    Assistant,
}

/// Optional lifecycle phase for assistant message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMessagePhase {
    Commentary,
    Final,
}

/// Content carried by a history message.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryMessageContent {
    Text(String),
    Image {
        source: HistoryMediaSource,
        detail: Option<HistoryImageDetail>,
    },
    Audio {
        source: HistoryMediaSource,
    },
}

/// Media represented without assuming a particular upload or URL mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMediaSource {
    Uri(String),
    Bytes { media_type: String, data: Arc<[u8]> },
}

/// Optional image-fidelity hint retained with image content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryImageDetail {
    Auto,
    Low,
    High,
    Original,
}

/// Plaintext reasoning exposed by the harness.
///
/// Opaque or encrypted continuation state is intentionally not part of this semantic value.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryReasoning {
    /// Ordered reasoning summary sections.
    pub summary: Vec<String>,
    /// Ordered plaintext reasoning sections, when available.
    pub content: Vec<String>,
}

/// Correlation identifier for one logical history tool call and its result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistoryToolCallId(pub String);

/// A tool invocation retained in conversation history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryToolCall {
    pub call_id: HistoryToolCallId,
    pub namespace: Option<String>,
    pub name: String,
    pub input: HistoryToolInput,
}

/// Tool-call input with its semantic JSON/text distinction preserved.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryToolInput {
    Json(Value),
    Text(String),
}

/// The result produced for a previous history tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryToolResult {
    pub call_id: HistoryToolCallId,
    pub content: Vec<HistoryToolResultContent>,
    /// `Some(true)` and `Some(false)` are explicit statuses; `None` means unknown.
    pub is_error: Option<bool>,
}

/// Content produced by a history tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryToolResultContent {
    Text(String),
    Json(Value),
    Image {
        source: HistoryMediaSource,
        detail: Option<HistoryImageDetail>,
    },
    Audio {
        source: HistoryMediaSource,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

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
