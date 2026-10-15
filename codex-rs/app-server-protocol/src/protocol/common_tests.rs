use super::*;
use crate::ReasoningContentPartAddedNotification;
use crate::item_event_to_server_notification;
use anyhow::Result;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ReasoningRawContentSectionStartedEvent;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn client_response_payload_serializes_without_an_intermediate_json_value() -> Result<()> {
    let payload = ClientResponsePayload::ThreadArchive(v2::ThreadArchiveResponse {});
    assert_eq!(serde_json::to_string(&payload)?, "{}");
    let Some(ClientResponse::ThreadArchive {
        request_id,
        response: _,
    }) = payload.into_client_response(RequestId::Integer(7))
    else {
        panic!("expected thread/archive client response");
    };
    assert_eq!(request_id, RequestId::Integer(7));
    Ok(())
}

#[test]
fn interrupt_conversation_payload_stays_jsonrpc_only() -> Result<()> {
    let payload = ClientResponsePayload::InterruptConversation(v1::InterruptConversationResponse {
        abort_reason: TurnAbortReason::Interrupted,
    });
    assert_eq!(
        serde_json::to_value(&payload)?,
        json!({
            "abortReason": "interrupted",
        })
    );
    assert!(
        payload
            .into_client_response(RequestId::Integer(8))
            .is_none()
    );
    Ok(())
}

#[test]
fn reasoning_content_part_added_is_experimental_and_serializes_with_wire_names() -> Result<()> {
    let notification =
        ServerNotification::ReasoningContentPartAdded(v2::ReasoningContentPartAddedNotification {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "reasoning-1".to_string(),
            content_index: 4,
        });

    assert_eq!(
        serde_json::to_value(&notification)?,
        json!({
            "method": "item/reasoning/contentPartAdded",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "reasoning-1",
                "contentIndex": 4,
            },
        })
    );
    assert_eq!(
        crate::experimental_api::ExperimentalApi::experimental_reason(&notification),
        Some("item/reasoning/contentPartAdded")
    );
    Ok(())
}

#[test]
fn raw_content_section_start_maps_to_content_part_added() {
    let notification = item_event_to_server_notification(
        EventMsg::ReasoningRawContentSectionStarted(ReasoningRawContentSectionStartedEvent {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            item_id: "reasoning-1".to_string(),
            content_index: 4,
        }),
        "thread-1",
        "turn-1",
    );

    match notification {
        ServerNotification::ReasoningContentPartAdded(payload) => assert_eq!(
            payload,
            ReasoningContentPartAddedNotification {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                item_id: "reasoning-1".to_string(),
                content_index: 4,
            }
        ),
        other => panic!("expected reasoning content part added, got {other:?}"),
    }
}
