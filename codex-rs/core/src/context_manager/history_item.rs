use codex_protocol::models::ResponseItem;

/// Provider-neutral tool-call families used by conversation-history normalization.
///
/// These values describe the pairing semantics the harness needs from history. They do not expose
/// Responses item variants, execution status strings, or provider item identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HistoryToolFamily {
    Function,
    ToolSearch,
    Custom,
}

/// Which side of a tool call/result pair an item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryToolSide {
    Call,
    Output,
}

/// Read-only pairing semantics projected from the current compatibility history item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryToolCorrelation<'a> {
    pub(crate) family: HistoryToolFamily,
    pub(crate) side: HistoryToolSide,
    pub(crate) call_id: &'a str,
    /// Some provider-owned outputs legitimately have no matching call in local history.
    pub(crate) counterpart_required: bool,
}

/// Projects the current Responses-shaped compatibility item into the smallest tool-correlation
/// vocabulary required by history normalization.
///
/// Keeping this mapping in one place lets normalization operate on harness semantics while the
/// persisted and model-visible compatibility representation remains unchanged during migration.
pub(crate) fn tool_correlation(item: &ResponseItem) -> Option<HistoryToolCorrelation<'_>> {
    match item {
        ResponseItem::FunctionCall { call_id, .. } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::Function,
            side: HistoryToolSide::Call,
            call_id,
            counterpart_required: true,
        }),
        // Local shell calls are paired with FunctionCallOutput in the existing Responses shape.
        ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::Function,
            side: HistoryToolSide::Call,
            call_id,
            counterpart_required: true,
        }),
        ResponseItem::FunctionCallOutput {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::Function,
            side: HistoryToolSide::Output,
            call_id,
            counterpart_required: true,
        }),
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::ToolSearch,
            side: HistoryToolSide::Call,
            call_id,
            counterpart_required: true,
        }),
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            execution,
            ..
        } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::ToolSearch,
            side: HistoryToolSide::Output,
            call_id,
            // Server-owned search outputs may arrive without a client-side call in history.
            counterpart_required: execution != "server",
        }),
        ResponseItem::CustomToolCall { call_id, .. } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::Custom,
            side: HistoryToolSide::Call,
            call_id,
            counterpart_required: true,
        }),
        ResponseItem::CustomToolCallOutput { call_id, .. } => Some(HistoryToolCorrelation {
            family: HistoryToolFamily::Custom,
            side: HistoryToolSide::Output,
            call_id,
            counterpart_required: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;

    #[test]
    fn function_call_and_output_share_neutral_correlation() {
        let call = ResponseItem::FunctionCall {
            id: None,
            name: "shell".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            encrypted_function_args: None,
            call_id: "call-1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        };
        let output = ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-1".to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text("ok".to_string()),
            internal_chat_message_metadata_passthrough: None,
        };

        let call = tool_correlation(&call).expect("call correlation");
        let output = tool_correlation(&output).expect("output correlation");
        assert_eq!(call.family, HistoryToolFamily::Function);
        assert_eq!(output.family, HistoryToolFamily::Function);
        assert_eq!(call.call_id, output.call_id);
        assert_eq!(call.side, HistoryToolSide::Call);
        assert_eq!(output.side, HistoryToolSide::Output);
        assert!(output.counterpart_required);
    }

    #[test]
    fn server_tool_search_output_does_not_require_local_call() {
        let output = ResponseItem::ToolSearchOutput {
            id: None,
            call_id: Some("search-1".to_string()),
            status: "completed".to_string(),
            execution: "server".to_string(),
            tools: Vec::new(),
            internal_chat_message_metadata_passthrough: None,
        };

        let correlation = tool_correlation(&output).expect("tool search correlation");
        assert_eq!(correlation.family, HistoryToolFamily::ToolSearch);
        assert_eq!(correlation.side, HistoryToolSide::Output);
        assert!(!correlation.counterpart_required);
    }

    #[test]
    fn missing_optional_call_ids_do_not_create_false_correlations() {
        let output = ResponseItem::FunctionCallOutput {
            id: None,
            call_id: None,
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text("legacy".to_string()),
            internal_chat_message_metadata_passthrough: None,
        };

        assert_eq!(tool_correlation(&output), None);
    }
}
