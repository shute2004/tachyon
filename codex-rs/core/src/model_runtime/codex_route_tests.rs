use super::*;

#[test]
fn codex_route_keeps_protocol_identity_independent_from_transport() {
    let http = codex_route(/*websocket_enabled*/ false);
    let websocket = codex_route(/*websocket_enabled*/ true);

    assert_eq!(http.protocol().id(), OPENAI_RESPONSES_PROTOCOL_ID);
    assert_eq!(websocket.protocol().id(), OPENAI_RESPONSES_PROTOCOL_ID);
    assert_eq!(http.transport(), ModelTransport::Http);
    assert_eq!(websocket.transport(), ModelTransport::WebSocket);
}

#[test]
fn fallback_state_changes_transport_without_changing_protocol() {
    let before_fallback = codex_route(/*websocket_enabled*/ true);
    let after_fallback = codex_route(/*websocket_enabled*/ false);

    assert_eq!(before_fallback.protocol(), after_fallback.protocol());
    assert_ne!(before_fallback.transport(), after_fallback.transport());
}
