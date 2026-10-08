use super::*;

#[test]
fn core_ir_types_are_tachyon_model_types_without_conversion() {
    let request: tachyon_model::ModelRequest = ModelRequest::default();
    let event: tachyon_model::ModelEvent = ModelEvent::Started;
    let result: tachyon_model::ModelToolResult = ModelToolResult {
        call_id: ModelToolCallId("call-1".to_string()),
        content: vec![ModelToolResultContent::Text("result".to_string())],
        is_error: Some(false),
    };

    assert_eq!(request, tachyon_model::ModelRequest::default());
    assert_eq!(event, tachyon_model::ModelEvent::Started);
    assert_eq!(
        result,
        tachyon_model::ModelToolResult {
            call_id: tachyon_model::ModelToolCallId("call-1".to_string()),
            content: vec![tachyon_model::ModelToolResultContent::Text(
                "result".to_string()
            )],
            is_error: Some(false),
        }
    );
}

#[test]
fn core_route_types_are_tachyon_model_types_without_conversion() {
    let provider_id: tachyon_model::route::ModelProviderId =
        crate::model_runtime::route::ModelProviderId::new("openai");
    let protocol: tachyon_model::route::ModelProtocol =
        crate::model_runtime::route::ModelProtocol::new("openai.responses");
    let transport: tachyon_model::route::ModelTransport =
        crate::model_runtime::route::ModelTransport::WebSocket;
    let route: tachyon_model::route::ModelRoute = crate::model_runtime::route::ModelRoute::new(
        provider_id.clone(),
        protocol.clone(),
        transport,
    );

    assert_eq!(route.provider_id(), &provider_id);
    assert_eq!(route.protocol(), &protocol);
    assert_eq!(route.transport(), transport);
}
