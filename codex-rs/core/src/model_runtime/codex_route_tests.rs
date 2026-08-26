use super::*;
use crate::model_runtime::route::ModelProviderId;

#[test]
fn session_startup_transport_tracks_websocket_capability() {
    let http = codex_transport(/*websocket_enabled*/ false);
    let websocket = codex_transport(/*websocket_enabled*/ true);

    assert_eq!(http, ModelTransport::Http);
    assert_eq!(websocket, ModelTransport::WebSocket);
}

#[test]
fn turn_route_binds_provider_identity_independently_from_protocol() {
    let openai = codex_route(
        ModelProviderId::new("openai"),
        /*websocket_enabled*/ false,
    );
    let compatible = codex_route(
        ModelProviderId::new("compatible-provider"),
        /*websocket_enabled*/ false,
    );

    assert_ne!(openai.provider_id(), compatible.provider_id());
    assert_eq!(openai.protocol(), compatible.protocol());
    assert_eq!(openai.protocol().id(), OPENAI_RESPONSES_PROTOCOL_ID);
    assert_eq!(openai.transport(), compatible.transport());
}

#[test]
fn fallback_changes_only_transport_for_provider_bound_route() {
    let before_fallback = codex_route(
        ModelProviderId::new("openai"),
        /*websocket_enabled*/ true,
    );
    let after_fallback = codex_route(
        ModelProviderId::new("openai"),
        /*websocket_enabled*/ false,
    );

    assert_eq!(before_fallback.provider_id(), after_fallback.provider_id());
    assert_eq!(before_fallback.protocol(), after_fallback.protocol());
    assert_ne!(before_fallback.transport(), after_fallback.transport());
}
