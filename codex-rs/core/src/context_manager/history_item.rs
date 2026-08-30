use codex_protocol::models::ResponseItem;

/// Compatibility-only pairing classes for the current Responses-shaped history payload.
///
/// These values describe which call/output variants share a pairing namespace in the current
/// compatibility representation. They are not a provider-neutral tool taxonomy and must not be
/// reused as the semantic classification for a future canonical `HistoryItem`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResponsesToolPairingClass {
    FunctionCallOutput,
    ToolSearchOutput,
    CustomToolCallOutput,
}

/// Which side of a tool call/result pair an item represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryToolSide {
    Call,
    Output,
}

/// Read-only correlation semantics projected from the current compatibility history item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryToolCorrelation<'a> {
    /// Migration-only discriminator needed to pair the current Responses-shaped variants.
    pub(crate) compatibility_pairing_class: ResponsesToolPairingClass,
    pub(crate) side: HistoryToolSide,
    pub(crate) call_id: &'a str,
    /// Some provider-owned outputs legitimately have no matching call in local history.
    pub(crate) local_counterpart_required: bool,
}

/// Projects the current Responses-shaped compatibility item into the correlation facts required by
/// history normalization plus an explicitly compatibility-only pairing discriminator.
///
/// Keeping this mapping in one place lets normalization avoid depending on individual Responses
/// variants for ordinary matching without turning the Responses pairing taxonomy into kernel
/// history semantics.
pub(crate) fn tool_correlation(item: &ResponseItem) -> Option<HistoryToolCorrelation<'_>> {
    match item {
        ResponseItem::FunctionCall { call_id, .. } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::FunctionCallOutput,
            side: HistoryToolSide::Call,
            call_id,
            local_counterpart_required: true,
        }),
        // Local shell calls are paired with FunctionCallOutput in the existing Responses shape.
        ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::FunctionCallOutput,
            side: HistoryToolSide::Call,
            call_id,
            local_counterpart_required: true,
        }),
        ResponseItem::FunctionCallOutput {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::FunctionCallOutput,
            side: HistoryToolSide::Output,
            call_id,
            local_counterpart_required: true,
        }),
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::ToolSearchOutput,
            side: HistoryToolSide::Call,
            call_id,
            local_counterpart_required: true,
        }),
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            execution,
            ..
        } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::ToolSearchOutput,
            side: HistoryToolSide::Output,
            call_id,
            // Server-owned search outputs may arrive without a client-side call in history.
            local_counterpart_required: execution != "server",
        }),
        ResponseItem::CustomToolCall { call_id, .. } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::CustomToolCallOutput,
            side: HistoryToolSide::Call,
            call_id,
            local_counterpart_required: true,
        }),
        ResponseItem::CustomToolCallOutput { call_id, .. } => Some(HistoryToolCorrelation {
            compatibility_pairing_class: ResponsesToolPairingClass::CustomToolCallOutput,
            side: HistoryToolSide::Output,
            call_id,
            local_counterpart_required: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::FunctionCallOutputPayload;

    #[test]
    fn function_call_and_output_share_responses_pairing_class() {
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
        assert_eq!(
            call.compatibility_pairing_class,
            ResponsesToolPairingClass::FunctionCallOutput
        );
        assert_eq!(
            output.compatibility_pairing_class,
            ResponsesToolPairingClass::FunctionCallOutput
        );
        assert_eq!(call.call_id, output.call_id);
        assert_eq!(call.side, HistoryToolSide::Call);
        assert_eq!(output.side, HistoryToolSide::Output);
        assert!(output.local_counterpart_required);
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
        assert_eq!(
            correlation.compatibility_pairing_class,
            ResponsesToolPairingClass::ToolSearchOutput
        );
        assert_eq!(correlation.side, HistoryToolSide::Output);
        assert!(!correlation.local_counterpart_required);
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
