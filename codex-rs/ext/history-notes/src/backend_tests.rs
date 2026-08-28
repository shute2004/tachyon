use codex_login::AuthHeaders;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::auth::BedrockApiKeyAuth;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_utils_output_truncation::TruncationPolicy;
use http::HeaderMap;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::ENCRYPTED_TOOL_ARGUMENTS_HEADER;
use super::HistoryNotesBackend;
use super::TOOL_OUTPUT_TRUNCATION_POLICY_HEADER;

#[tokio::test]
async fn reports_provider_resolution_error_before_auth_resolution() {
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-west-1".to_string(),
        }));
    let backend = HistoryNotesBackend::new(create_model_provider(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        Some(auth_manager),
    ));

    let error = backend
        .call(
            "alpha/notes/v2/read_file",
            "session-123",
            "/root",
            json!({"path": "notes.md"}),
            TruncationPolicy::Bytes(1024),
        )
        .await
        .expect_err("unsupported Bedrock region should fail provider resolution");

    assert_eq!(
        error,
        "Unable to perform operation: Could not resolve the backend provider."
    );
}

#[tokio::test]
async fn reports_auth_resolution_error_after_provider_resolution() {
    let provider_info =
        ModelProviderInfo::create_openai_provider(Some("https://example.test/v1".to_string()));
    let backend = HistoryNotesBackend::new(create_model_provider(
        provider_info,
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
                api_key: "bedrock-api-key-test".to_string(),
                region: "us-east-1".to_string(),
            }),
        )),
    ));

    let error = backend
        .call(
            "alpha/notes/v2/read_file",
            "session-123",
            "/root",
            json!({"path": "notes.md"}),
            TruncationPolicy::Bytes(1024),
        )
        .await
        .expect_err("Bedrock auth should fail for the OpenAI provider");

    assert_eq!(
        error,
        "Unable to perform operation: Could not resolve backend authentication."
    );
}

#[tokio::test]
async fn routes_through_codex_backend_and_injects_trusted_session_agent_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/backend-api/codex/alpha/notes/v2/read_file"))
        .and(header("x-openai-actor-authorization", "actor-biscuit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "encrypted_output": "enc_payload"
        })))
        .mount(&server)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-openai-actor-authorization",
        HeaderValue::from_static("actor-biscuit"),
    );
    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::Headers(AuthHeaders::new(headers)));
    let provider = create_model_provider(
        ModelProviderInfo::create_openai_provider(Some(format!(
            "{}/backend-api/codex",
            server.uri()
        ))),
        Some(auth_manager),
    );
    let backend = HistoryNotesBackend::new(provider);

    let response = backend
        .call(
            "alpha/notes/v2/read_file",
            "session-123",
            "/root/worker",
            json!({
                "path": "notes.md",
                "context": {
                    "session_id": "spoofed-session",
                    "current_agent_name": "/root/spoofed",
                }
            }),
            TruncationPolicy::Bytes(1024),
        )
        .await
        .expect("History request should succeed");

    assert_eq!(response, json!({"encrypted_output": "enc_payload"}));
    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 1);
    let expected_truncation_policy =
        serde_json::to_string(&TruncationPolicy::Bytes(1024)).expect("serialize truncation policy");
    let expected_truncation_policy_header =
        HeaderValue::from_bytes(expected_truncation_policy.as_bytes())
            .expect("valid truncation policy header");
    assert_eq!(
        requests[0]
            .headers
            .get(TOOL_OUTPUT_TRUNCATION_POLICY_HEADER),
        Some(&expected_truncation_policy_header)
    );
    assert!(
        requests[0]
            .headers
            .get(ENCRYPTED_TOOL_ARGUMENTS_HEADER)
            .is_none()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).expect("JSON body"),
        json!({
            "path": "notes.md",
            "context": {
                "session_id": "session-123",
                "current_agent_name": "/root/worker",
            }
        })
    );
}

#[tokio::test]
async fn marks_encrypted_history_and_notes_arguments_without_changing_the_json_body() {
    let server = MockServer::start().await;
    let cases = [
        (
            "history/v2/search_contents",
            json!({"query": "encrypted-query"}),
        ),
        (
            "notes/v2/search_contents",
            json!({"query": "encrypted-query"}),
        ),
        (
            "notes/v2/append_to_file",
            json!({"path": "notes.md", "text": "encrypted-text"}),
        ),
        (
            "notes/v2/write_file",
            json!({"path": "notes.md", "text": "encrypted-text"}),
        ),
    ];
    for (route, arguments) in &cases {
        Mock::given(method("POST"))
            .and(path(format!("/backend-api/codex/alpha/{route}")))
            .and(header(ENCRYPTED_TOOL_ARGUMENTS_HEADER, "true"))
            .and(body_partial_json(arguments.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
    }

    let auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::Headers(AuthHeaders::new(HeaderMap::new())));
    let backend = HistoryNotesBackend::new(create_model_provider(
        ModelProviderInfo::create_openai_provider(Some(format!(
            "{}/backend-api/codex",
            server.uri()
        ))),
        Some(auth_manager),
    ));

    for (route, arguments) in cases {
        backend
            .call(
                &format!("alpha/{route}"),
                "session-123",
                "/root",
                arguments.clone(),
                TruncationPolicy::Bytes(1024),
            )
            .await
            .expect("encrypted argument request should succeed");
    }
}
