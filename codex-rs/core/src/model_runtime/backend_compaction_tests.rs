use std::sync::Arc;
use std::sync::Mutex;

use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use pretty_assertions::assert_eq;
use tachyon_model::ModelContent;
use tachyon_model::ModelInputItem;
use tachyon_model::ModelMessage;
use tachyon_model::ModelMessagePhase;
use tachyon_model::ModelMessageRole;
use tachyon_model::ModelRequest;
use tachyon_model::ModelUsage;
use tachyon_model::backend::ModelBackend;
use tachyon_model::backend::ModelBackendError;
use tachyon_model::backend::ModelBackendFuture;
use tachyon_model::backend::ModelCompactionItem;
use tachyon_model::backend::ModelCompactionResult;
use tachyon_model::backend::ModelEventStream;
use tachyon_model::backend::ModelTurnBackend;
use tachyon_model::route::ModelProtocol;
use tachyon_model::route::ModelProviderId;
use tachyon_model::route::ModelRoute;
use tachyon_model::route::ModelTransport;

use super::ModelRuntime;

#[derive(Debug, Clone, PartialEq)]
struct RecordedCompactionCall {
    turn_id: usize,
    route: ModelRoute,
    model_id: String,
    request: ModelRequest,
}

#[derive(Debug, Default)]
struct FakeBackendState {
    turns_created: usize,
    compaction_calls: Vec<RecordedCompactionCall>,
    stream_calls: usize,
}

#[derive(Debug, Clone)]
struct FakeBackend {
    state: Arc<Mutex<FakeBackendState>>,
    supports_compaction: bool,
    route_provider_override: Option<ModelProviderId>,
    outcome: std::result::Result<ModelCompactionResult, ModelBackendError>,
}

impl ModelBackend for FakeBackend {
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        let turn_id = {
            let mut state = self.state.lock().expect("fake backend lock poisoned");
            let turn_id = state.turns_created;
            state.turns_created += 1;
            turn_id
        };
        let route = ModelRoute::new(
            self.route_provider_override.clone().unwrap_or(provider_id),
            ModelProtocol::new("test.protocol"),
            ModelTransport::Http,
        );
        Box::new(FakeTurnBackend {
            turn_id,
            route,
            state: Arc::clone(&self.state),
            outcome: self.outcome.clone(),
        })
    }

    fn supports_compaction(&self) -> bool {
        self.supports_compaction
    }
}

#[derive(Debug)]
struct FakeTurnBackend {
    turn_id: usize,
    route: ModelRoute,
    state: Arc<Mutex<FakeBackendState>>,
    outcome: std::result::Result<ModelCompactionResult, ModelBackendError>,
}

impl ModelTurnBackend for FakeTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn stream<'a>(
        &'a mut self,
        _model_id: &'a str,
        _request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, std::result::Result<ModelEventStream, ModelBackendError>> {
        self.state
            .lock()
            .expect("fake backend lock poisoned")
            .stream_calls += 1;
        let result: std::result::Result<ModelEventStream, ModelBackendError> = Err(
            ModelBackendError::UnsupportedRequest("unexpected sampling call".to_string()),
        );
        Box::pin(std::future::ready(result))
    }

    fn compact<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, std::result::Result<ModelCompactionResult, ModelBackendError>> {
        self.state
            .lock()
            .expect("fake backend lock poisoned")
            .compaction_calls
            .push(RecordedCompactionCall {
                turn_id: self.turn_id,
                route: self.route.clone(),
                model_id: model_id.to_string(),
                request: request.clone(),
            });
        Box::pin(std::future::ready(self.outcome.clone()))
    }
}

fn fake_runtime(
    supports_compaction: bool,
    route_provider_override: Option<ModelProviderId>,
    outcome: std::result::Result<ModelCompactionResult, ModelBackendError>,
) -> (ModelRuntime, Arc<Mutex<FakeBackendState>>) {
    let state = Arc::new(Mutex::new(FakeBackendState::default()));
    let backend = FakeBackend {
        state: Arc::clone(&state),
        supports_compaction,
        route_provider_override,
        outcome,
    };
    (ModelRuntime::from_backend(Arc::new(backend)), state)
}

fn request() -> ModelRequest {
    ModelRequest {
        instructions: "test instructions".to_string(),
        input: vec![ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![ModelContent::Text("original input".to_string())],
        })],
        ..ModelRequest::default()
    }
}

fn compaction_result() -> ModelCompactionResult {
    ModelCompactionResult {
        history: vec![
            ModelCompactionItem::RetainedInput { index: 0 },
            ModelCompactionItem::NewItem(ModelInputItem::Message(ModelMessage {
                role: ModelMessageRole::Assistant,
                phase: Some(ModelMessagePhase::Final),
                content: vec![ModelContent::Text("summary".to_string())],
            })),
        ],
        usage: Some(ModelUsage {
            input_tokens: 11,
            output_tokens: 4,
            cached_input_tokens: Some(3),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(1),
            total_tokens: Some(15),
        }),
    }
}

fn assert_invalid_request(error: &CodexErr, expected: &str) {
    assert!(
        matches!(
            error.details(),
            CodexErrorDetails::InvalidRequest(message) if message == expected
        ),
        "expected InvalidRequest with the requested message"
    );
}

#[tokio::test]
async fn compaction_capability_queries_do_not_construct_turns() {
    for supports_compaction in [false, true] {
        let (runtime, state) = fake_runtime(
            supports_compaction,
            /*route_provider_override*/ None,
            Ok(compaction_result()),
        );

        assert_eq!(runtime.supports_backend_compaction(), supports_compaction);
        assert_eq!(
            state
                .lock()
                .expect("fake backend lock poisoned")
                .turns_created,
            0
        );
    }
}

#[tokio::test]
async fn backend_compaction_reuses_the_bound_turn_and_preserves_request_and_result() {
    let expected_result = compaction_result();
    let (runtime, state) = fake_runtime(
        /*supports_compaction*/ true,
        /*route_provider_override*/ None,
        Ok(expected_result.clone()),
    );
    let request = request();
    let mut first_turn = runtime.begin_turn_for_provider("provider-a");

    let first_result = first_turn
        .compact_backend_request("selected-model", &request)
        .await
        .expect("first compaction succeeds");
    let second_result = first_turn
        .compact_backend_request("selected-model", &request)
        .await
        .expect("same-turn compaction succeeds");

    let mut second_turn = runtime.begin_turn_for_provider("provider-b");
    let fresh_turn_result = second_turn
        .compact_backend_request("other-model", &request)
        .await
        .expect("fresh-turn compaction succeeds");

    assert_eq!(first_result, expected_result);
    assert_eq!(second_result, expected_result);
    assert_eq!(fresh_turn_result, expected_result);

    let calls = state
        .lock()
        .expect("fake backend lock poisoned")
        .compaction_calls
        .clone();
    assert_eq!(
        calls,
        vec![
            RecordedCompactionCall {
                turn_id: 0,
                route: ModelRoute::new(
                    ModelProviderId::new("provider-a"),
                    ModelProtocol::new("test.protocol"),
                    ModelTransport::Http,
                ),
                model_id: "selected-model".to_string(),
                request: request.clone(),
            },
            RecordedCompactionCall {
                turn_id: 0,
                route: ModelRoute::new(
                    ModelProviderId::new("provider-a"),
                    ModelProtocol::new("test.protocol"),
                    ModelTransport::Http,
                ),
                model_id: "selected-model".to_string(),
                request: request.clone(),
            },
            RecordedCompactionCall {
                turn_id: 1,
                route: ModelRoute::new(
                    ModelProviderId::new("provider-b"),
                    ModelProtocol::new("test.protocol"),
                    ModelTransport::Http,
                ),
                model_id: "other-model".to_string(),
                request,
            },
        ]
    );
    assert_eq!(
        state
            .lock()
            .expect("fake backend lock poisoned")
            .stream_calls,
        0
    );
}

#[tokio::test]
async fn backend_compaction_rejects_a_mismatched_provider_before_calling_backend() {
    let (runtime, state) = fake_runtime(
        /*supports_compaction*/ true,
        Some(ModelProviderId::new("route-provider")),
        Ok(compaction_result()),
    );
    let request = request();
    let mut turn = runtime.begin_turn_for_provider("configured-provider");

    let error = turn
        .compact_backend_request("selected-model", &request)
        .await
        .expect_err("provider mismatch must be rejected");

    assert_invalid_request(
        &error,
        "configured model provider `configured-provider` does not match model backend route provider `route-provider`",
    );
    let state = state.lock().expect("fake backend lock poisoned");
    assert!(state.compaction_calls.is_empty());
    assert_eq!(state.stream_calls, 0);
}

#[tokio::test]
async fn backend_compaction_errors_are_mapped_without_sampling_fallback() {
    enum ExpectedError {
        InvalidRequest(&'static str),
        TurnAborted,
    }

    let cases = [
        (
            ModelBackendError::UnsupportedRequest("compaction unavailable".to_string()),
            ExpectedError::InvalidRequest(
                "unsupported canonical model request: compaction unavailable",
            ),
        ),
        (ModelBackendError::Cancelled, ExpectedError::TurnAborted),
        (
            ModelBackendError::Failed {
                message: "compaction failed".to_string(),
                retryable: false,
            },
            ExpectedError::InvalidRequest("compaction failed"),
        ),
    ];

    for (backend_error, expected_error) in cases {
        let (runtime, state) = fake_runtime(
            /*supports_compaction*/ true,
            /*route_provider_override*/ None,
            Err(backend_error),
        );
        let mut turn = runtime.begin_turn_for_provider("provider-a");
        let error = turn
            .compact_backend_request("selected-model", &request())
            .await
            .expect_err("backend error must be propagated");

        match expected_error {
            ExpectedError::InvalidRequest(message) => assert_invalid_request(&error, message),
            ExpectedError::TurnAborted => {
                assert!(matches!(error.details(), CodexErrorDetails::TurnAborted))
            }
        }
        let state = state.lock().expect("fake backend lock poisoned");
        assert_eq!(state.compaction_calls.len(), 1);
        assert_eq!(state.stream_calls, 0);
    }
}

#[tokio::test]
async fn backend_compaction_rejects_invalid_retained_input_references() {
    let invalid_results = [
        (
            ModelCompactionResult {
                history: vec![
                    ModelCompactionItem::RetainedInput { index: 0 },
                    ModelCompactionItem::RetainedInput { index: 0 },
                ],
                usage: None,
            },
            "more than once",
        ),
        (
            ModelCompactionResult {
                history: vec![ModelCompactionItem::RetainedInput { index: usize::MAX }],
                usage: None,
            },
            "input length is 1",
        ),
    ];

    for (invalid_result, expected_message) in invalid_results {
        let unchanged = invalid_result.clone();
        let (runtime, state) = fake_runtime(
            /*supports_compaction*/ true,
            /*route_provider_override*/ None,
            Ok(invalid_result.clone()),
        );
        let mut turn = runtime.begin_turn_for_provider("provider-a");
        let error = turn
            .compact_backend_request("selected-model", &request())
            .await
            .expect_err("invalid retained references must be rejected");

        assert!(matches!(
            error.details(),
            CodexErrorDetails::InvalidRequest(message) if message.contains(expected_message)
        ));
        assert_eq!(invalid_result, unchanged);
        let state = state.lock().expect("fake backend lock poisoned");
        assert_eq!(state.compaction_calls.len(), 1);
        assert_eq!(state.stream_calls, 0);
    }
}
