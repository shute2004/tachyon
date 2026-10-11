use super::ServerNotificationThreadTarget;
use super::server_notification_thread_target;
use codex_app_server_protocol::ReasoningContentPartAddedNotification;
use codex_app_server_protocol::ServerNotification;
use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;

fn reasoning_content_part_added(thread_id: String) -> ServerNotification {
    ServerNotification::ReasoningContentPartAdded(ReasoningContentPartAddedNotification {
        thread_id,
        turn_id: "turn-1".to_string(),
        item_id: "reasoning-1".to_string(),
        content_index: 2,
    })
}

#[test]
fn reasoning_content_part_added_routes_to_owning_thread() {
    let thread_id = ThreadId::new();
    let notification = reasoning_content_part_added(thread_id.to_string());

    assert_eq!(
        server_notification_thread_target(&notification),
        ServerNotificationThreadTarget::Thread(thread_id)
    );
}

#[test]
fn reasoning_content_part_added_with_invalid_thread_id_is_reported() {
    let notification = reasoning_content_part_added("invalid-thread-id".to_string());

    assert_eq!(
        server_notification_thread_target(&notification),
        ServerNotificationThreadTarget::InvalidThreadId("invalid-thread-id".to_string())
    );
}
