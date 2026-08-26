use super::*;
use crate::model_runtime::route::ModelProviderId;
use crate::model_runtime::route::ModelRoute;

#[test]
fn codex_unresolved_route_keeps_protocol_identity_independent_from_transport() {
    let http = codex_route(/*websocket_enabled*/ false);
    let websocket = codex_route(/*websocket_enabled*/ true);

    assert_eq!(http.protocol().id(), OPENAI_RESPONSES_PROTOCOL_ID);
    assert_eq!(websocket.protocol().id(), OPENAI_RESPONSES_PROTOCOL_ID);
    assert_eq!(http.transport(), ModelTransport::Http);
    assert_eq!(websocket.transport(), ModelTransport::WebSocket);
}

#[test]
fn fallback_state_changes_transport_without_fabricating_provider_identity() {
    let before_fallback = codex_route(/*websocket_enabled*/ true);
    let after_fallback = codex_route(/*websocket_enabled*/ false);

    assert_eq!(before_fallback.protocol(), after_fallback.protocol());
    assert_ne!(before_fallback.transport(), after_fallback.transport());
}

#[test]
fn identified_routes_keep_provider_and_protocol_as_independent_dimensions() {
    let openai = ModelRoute::new(
        ModelProviderId::new("openai"),
        ModelProtocol::new(OPENAI_RESPONSES_PROTOCOL_ID),
        ModelTransport::Http,
    );
    let compatible = ModelRoute::new(
        ModelProviderId::new("compatible-provider"),
        ModelProtocol::new(OPENAI_RESPONSES_PROTOCOL_ID),
        ModelTransport::Http,
    );

    assert_ne!(openai.provider_id(), compatible.provider_id());
    assert_eq!(openai.protocol(), compatible.protocol());
    assert_eq!(openai.transport(), compatible.transport());
}
