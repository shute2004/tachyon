use super::build_test_config;
use super::make_session_and_context;
use super::raw_history_items;
use crate::agent::control::SpawnAgentForkMode;
use crate::agent::control::SpawnAgentOptions;
use crate::client_common::Prompt;
use crate::client_common::ResponseStream;
use crate::model_runtime::ModelRuntime;
use crate::model_runtime::SamplingModelStream;
use crate::model_runtime::model_backend_error_to_codex;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::thread_manager::StartThreadOptions;
use crate::thread_manager::ThreadManager;
use codex_features::Feature;
use codex_login::CodexAuth;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::models::ContentItem;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::TokenCountEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout_trace::InferenceTraceContext;
use core_test_support::test_codex::TurnInputRequest as ExternalTurnInputRequest;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tachyon_model::ModelCompletion;
use tachyon_model::ModelContent;
use tachyon_model::ModelEvent;
use tachyon_model::ModelInputItem;
use tachyon_model::ModelItemId;
use tachyon_model::ModelMessagePhase;
use tachyon_model::ModelOutputItem;
use tachyon_model::ModelOutputItemStart;
use tachyon_model::ModelRequest;
use tachyon_model::ModelToolCall;
use tachyon_model::ModelToolCallId;
use tachyon_model::ModelToolInput;
use tachyon_model::ModelToolInputKind;
use tachyon_model::ModelToolSpec;
use tachyon_model::ModelUsage;
use tachyon_model::backend::ModelBackend;
use tachyon_model::backend::ModelBackendError;
use tachyon_model::backend::ModelBackendFuture;
use tachyon_model::backend::ModelEventSource;
use tachyon_model::backend::ModelEventStream;
use tachyon_model::backend::ModelTurnBackend;
use tachyon_model::route::ModelProtocol;
use tachyon_model::route::ModelProviderId;
use tachyon_model::route::ModelRoute;
use tachyon_model::route::ModelTransport;

#[derive(Clone, Debug)]
struct RecordedRequest {
    turn_index: usize,
    model_id: String,
    request: ModelRequest,
}

#[derive(Debug, Default)]
struct BackendState {
    turn_count: usize,
    requested_provider_ids: Vec<String>,
    requests: Vec<RecordedRequest>,
    child_session_factory_calls: usize,
    child_sessions: Vec<Arc<Mutex<BackendState>>>,
}

#[derive(Clone, Debug)]
struct FakeBackend {
    state: Arc<Mutex<BackendState>>,
    route_provider_override: Option<String>,
    close_without_completion: bool,
    simple_child_answer: bool,
}

impl FakeBackend {
    fn new(route_provider_override: Option<&str>) -> Self {
        Self {
            state: Arc::new(Mutex::new(BackendState::default())),
            route_provider_override: route_provider_override.map(str::to_string),
            close_without_completion: false,
            simple_child_answer: false,
        }
    }

    fn closing_without_completion() -> Self {
        Self {
            close_without_completion: true,
            ..Self::new(/*route_provider_override*/ None)
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().expect("backend state").requests.clone()
    }

    fn turn_count(&self) -> usize {
        self.state.lock().expect("backend state").turn_count
    }

    fn child_session_factory_calls(&self) -> usize {
        self.state
            .lock()
            .expect("backend state")
            .child_session_factory_calls
    }

    fn child_sessions(&self) -> Vec<Arc<Mutex<BackendState>>> {
        self.state
            .lock()
            .expect("backend state")
            .child_sessions
            .clone()
    }
}

impl ModelBackend for FakeBackend {
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        let turn_index = {
            let mut state = self.state.lock().expect("backend state");
            let turn_index = state.turn_count;
            state.turn_count += 1;
            state
                .requested_provider_ids
                .push(provider_id.id().to_string());
            turn_index
        };
        let route_provider = self
            .route_provider_override
            .as_deref()
            .map(ModelProviderId::new)
            .unwrap_or(provider_id);
        Box::new(FakeTurnBackend {
            route: ModelRoute::new(
                route_provider,
                ModelProtocol::new("test.protocol"),
                ModelTransport::Http,
            ),
            state: Arc::clone(&self.state),
            turn_index,
            request_index: 0,
            close_without_completion: self.close_without_completion,
            simple_answer: self.simple_child_answer,
        })
    }

    fn new_child_session(
        &self,
    ) -> ModelBackendFuture<'_, Result<Arc<dyn ModelBackend>, ModelBackendError>> {
        let parent_state = Arc::clone(&self.state);
        let route_provider_override = self.route_provider_override.clone();
        Box::pin(async move {
            let mut parent_state_guard = parent_state.lock().expect("backend state");
            parent_state_guard.child_session_factory_calls += 1;
            let child_state = Arc::new(Mutex::new(BackendState::default()));
            parent_state_guard
                .child_sessions
                .push(Arc::clone(&child_state));
            drop(parent_state_guard);
            Ok(Arc::new(Self {
                state: child_state,
                route_provider_override,
                close_without_completion: false,
                simple_child_answer: true,
            }) as Arc<dyn ModelBackend>)
        })
    }
}

struct FakeTurnBackend {
    route: ModelRoute,
    state: Arc<Mutex<BackendState>>,
    turn_index: usize,
    request_index: usize,
    close_without_completion: bool,
    simple_answer: bool,
}

impl ModelTurnBackend for FakeTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn stream<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>> {
        let request_index = self.request_index;
        self.request_index += 1;
        self.state
            .lock()
            .expect("backend state")
            .requests
            .push(RecordedRequest {
                turn_index: self.turn_index,
                model_id: model_id.to_string(),
                request: request.clone(),
            });
        let events = if self.close_without_completion {
            vec![ModelEvent::Started]
        } else if self.simple_answer {
            child_answer_events(self.turn_index, request_index)
        } else {
            events_for_request(self.turn_index, request_index)
        };
        Box::pin(async move {
            let source = FakeEventSource {
                events: events.into_iter().map(Ok).collect(),
            };
            Ok(Box::pin(source) as ModelEventStream)
        })
    }
}

fn child_answer_events(turn_index: usize, request_index: usize) -> Vec<ModelEvent> {
    let id = ModelItemId(format!("child-answer-{turn_index}-{request_index}"));
    vec![
        ModelEvent::Started,
        ModelEvent::OutputItemStarted(ModelOutputItemStart::Message {
            id: id.clone(),
            phase: Some(ModelMessagePhase::Final),
        }),
        ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
            id,
            phase: Some(ModelMessagePhase::Final),
            content: vec![ModelContent::Text("child backend answer".to_string())],
        }),
        completion(/*input_tokens*/ 3, /*total_tokens*/ 3, Some(true)),
    ]
}

struct FakeEventSource {
    events: VecDeque<Result<ModelEvent, ModelBackendError>>,
}

impl ModelEventSource for FakeEventSource {
    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<ModelEvent, ModelBackendError>>> {
        Poll::Ready(self.get_mut().events.pop_front())
    }
}

fn events_for_request(turn_index: usize, request_index: usize) -> Vec<ModelEvent> {
    if turn_index == 0 && request_index == 0 {
        return vec![
            ModelEvent::Started,
            ModelEvent::OutputItemStarted(ModelOutputItemStart::ToolCall {
                id: ModelItemId("item-plan".to_string()),
                call_id: ModelToolCallId("call-plan".to_string()),
                namespace: None,
                name: "update_plan".to_string(),
                input_kind: ModelToolInputKind::Json,
            }),
            ModelEvent::OutputItemCompleted(ModelOutputItem::ToolCall {
                id: ModelItemId("item-plan".to_string()),
                call: ModelToolCall {
                    call_id: ModelToolCallId("call-plan".to_string()),
                    namespace: None,
                    name: "update_plan".to_string(),
                    input: ModelToolInput::Json(serde_json::json!({
                        "plan": [{
                            "step": "Exercise canonical backend",
                            "status": "in_progress"
                        }]
                    })),
                },
            }),
            completion(
                /*input_tokens*/ 10,
                /*total_tokens*/ 12,
                Some(false),
            ),
        ];
    }

    let answer = if turn_index == 0 {
        "first turn answer"
    } else {
        "second turn answer"
    };
    vec![
        ModelEvent::Started,
        ModelEvent::OutputItemStarted(ModelOutputItemStart::Message {
            id: ModelItemId(format!("item-final-{turn_index}-{request_index}")),
            phase: Some(ModelMessagePhase::Final),
        }),
        ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
            id: ModelItemId(format!("item-final-{turn_index}-{request_index}")),
            phase: Some(ModelMessagePhase::Final),
            content: vec![ModelContent::Text(answer.to_string())],
        }),
        completion(/*input_tokens*/ 6, /*total_tokens*/ 6, Some(true)),
    ]
}

fn completion(input_tokens: u64, total_tokens: u64, end_turn: Option<bool>) -> ModelEvent {
    ModelEvent::Completed(ModelCompletion {
        usage: Some(ModelUsage {
            input_tokens,
            output_tokens: 2,
            total_tokens: Some(total_tokens),
            ..ModelUsage::default()
        }),
        end_turn,
    })
}

async fn wait_for_turn_end(thread: &crate::CodexThread) -> Vec<EventMsg> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut events = Vec::new();
        loop {
            let event = thread
                .next_event()
                .await
                .expect("thread event stream should stay open");
            let ended = matches!(
                &event.msg,
                EventMsg::TurnComplete(_) | EventMsg::TurnAborted(_)
            );
            events.push(event.msg);
            if ended {
                return events;
            }
        }
    })
    .await
    .expect("timed out waiting for turn end")
}

async fn submit_user_turn(thread: &crate::CodexThread, text: &str) {
    thread
        .start_or_steer_turn(ExternalTurnInputRequest::user_input(vec![
            UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            },
        ]))
        .await
        .expect("submit turn");
}

async fn canonical_backend_test_config(codex_home: &std::path::Path) -> crate::config::Config {
    let mut config = build_test_config(codex_home).await;
    config.ephemeral = true;
    config.update_plan_enabled = true;
    config
        .web_search_mode
        .set(WebSearchMode::Disabled)
        .expect("disable web search for canonical backend tests");
    config
}

async fn canonical_eof_test_config(codex_home: &std::path::Path) -> crate::config::Config {
    let mut config = canonical_backend_test_config(codex_home).await;
    config
        .features
        .disable(Feature::ShellTool)
        .expect("disable shell tool for bounded EOF test");
    config
        .features
        .disable(Feature::ViewImage)
        .expect("disable view image for bounded EOF test");
    config
        .features
        .disable(Feature::Collab)
        .expect("disable collaboration for bounded EOF test");
    config
}

fn find_model_function<'a>(tools: &'a [ModelToolSpec], name: &str) -> Option<&'a ModelToolSpec> {
    tools.iter().find_map(|tool| match tool {
        ModelToolSpec::Function {
            name: tool_name, ..
        } if tool_name == name => Some(tool),
        ModelToolSpec::Namespace {
            tools: children, ..
        } => find_model_function(children, name),
        ModelToolSpec::Function { .. } | ModelToolSpec::Freeform { .. } => None,
    })
}

fn assert_successful_turn(events: &[EventMsg], label: &str) {
    let completion = events.iter().find_map(|event| match event {
        EventMsg::TurnComplete(completion) => Some(completion),
        _ => None,
    });
    let error_message = completion
        .and_then(|completion| {
            completion
                .error
                .as_ref()
                .map(|error| error.message.as_str())
        })
        .or_else(|| {
            events.iter().find_map(|event| match event {
                EventMsg::Error(error) => Some(error.message.as_str()),
                _ => None,
            })
        })
        .unwrap_or("missing TurnComplete event");
    assert!(
        completion.is_some_and(|completion| completion.error.is_none()),
        "{label} did not complete successfully: {error_message}"
    );
}

async fn stream_error_for_backend(backend: Arc<FakeBackend>) -> CodexErr {
    let (session, turn_context) = make_session_and_context().await;
    let responses_metadata = session
        .responses_metadata(&turn_context, CodexResponsesRequestKind::Turn)
        .await;
    let trace = InferenceTraceContext::disabled();
    let prompt = Prompt::default();
    let mut runtime = ModelRuntime::from_backend(backend)
        .begin_turn_for_provider(turn_context.config.model_provider_id.clone());
    runtime
        .stream_migrating_request(
            None,
            &prompt,
            turn_context.model_info(),
            &session.services.session_telemetry,
            None,
            ReasoningSummary::Auto,
            None,
            &responses_metadata,
            &trace,
        )
        .await
        .err()
        .expect("canonical stream setup should fail")
}

#[tokio::test]
async fn injected_backend_runs_tool_followup_and_fresh_next_turn() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_backend_test_config(codex_home.path()).await;
    let model_id = config.model.clone().expect("configured model");
    let provider_id = config.model_provider_id.clone();
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    assert!(
        options
            .thread_extension_init
            .get::<ModelRuntime>()
            .is_some()
    );

    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    assert!(
        started
            .thread
            .thread_extension_data()
            .get::<ModelRuntime>()
            .is_some()
    );

    submit_user_turn(&started.thread, "run a harmless plan update").await;
    let first_turn_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&first_turn_events, "first fake-backend turn");
    assert!(
        first_turn_events
            .iter()
            .any(|event| matches!(event, EventMsg::PlanUpdate(_)))
    );
    assert!(first_turn_events.iter().any(|event| matches!(
        event,
        EventMsg::TokenCount(TokenCountEvent {
            info: Some(info),
            ..
        }) if info.last_token_usage.total_tokens > 0
    )));
    assert!(
        !first_turn_events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );

    submit_user_turn(&started.thread, "continue after the first turn").await;
    let second_turn_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&second_turn_events, "second fake-backend turn");

    let requests = backend.requests();
    assert_eq!(
        backend.turn_count(),
        2,
        "each user turn gets a fresh handle"
    );
    assert_eq!(
        backend
            .state
            .lock()
            .expect("backend state")
            .requested_provider_ids,
        vec![provider_id.clone(), provider_id]
    );
    assert_eq!(
        requests.len(),
        3,
        "tool follow-up reuses the first turn handle"
    );
    assert_eq!(requests[0].turn_index, 0);
    assert_eq!(requests[1].turn_index, 0);
    assert_eq!(requests[2].turn_index, 1);
    assert!(requests.iter().all(|request| request.model_id == model_id));
    let initial_tools = &requests.first().expect("initial request").request.tools;
    assert!(find_model_function(initial_tools, "update_plan").is_some());
    match find_model_function(initial_tools, "view_image") {
        Some(ModelToolSpec::Function {
            output_schema: Some(output_schema),
            ..
        }) => assert_eq!(output_schema["type"], "object"),
        _ => panic!("expected the local view_image declaration and output schema"),
    }
    assert!(requests[1].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::ToolResult(result) if result.call_id.0 == "call-plan"
    )));

    let history = raw_history_items(&started.thread.session.clone_history().await);
    assert!(history.iter().any(|item| matches!(
        item,
        ResponseItem::FunctionCall { call_id, .. } if call_id == "call-plan"
    )));
    assert!(history.iter().any(|item| matches!(
        item,
        ResponseItem::FunctionCallOutput { call_id: Some(call_id), .. }
            if call_id == "call-plan"
    )));
    assert!(history.iter().any(|item| matches!(
        item,
        ResponseItem::Message {
            role,
            phase: Some(MessagePhase::FinalAnswer),
            content,
            ..
        } if role == "assistant" && content.iter().any(|part| matches!(
            part,
            ContentItem::OutputText { text } if text == "second turn answer"
        ))
    )));

    started
        .thread
        .shutdown_and_wait()
        .await
        .expect("shutdown test thread");
}

#[tokio::test]
async fn codex_runtime_leaves_child_sessions_on_the_legacy_path() {
    let (session, _) = make_session_and_context().await;
    assert!(
        session
            .services
            .model_runtime()
            .new_child_session()
            .await
            .expect("Codex runtime preserves its legacy child path")
            .is_none()
    );
}

#[tokio::test]
async fn delegated_children_get_independent_backends_for_spawn_and_full_history_fork() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let mut config = canonical_backend_test_config(codex_home.path()).await;
    // Full-history forks load their source from the persistent thread store.
    config.ephemeral = false;
    let parent_backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(parent_backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let parent = manager.start_thread(options).await.expect("start parent");

    submit_user_turn(&parent.thread, "write parent history").await;
    let parent_events = wait_for_turn_end(&parent.thread).await;
    assert_successful_turn(&parent_events, "parent canonical turn");

    let source = || {
        Some(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: parent.thread_id,
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
        }))
    };
    let collaboration = parent
        .thread
        .session
        .services
        .agent_control
        .spawn_agent_with_metadata(
            config.clone(),
            vec![UserInput::Text {
                text: "answer as a delegated child".to_string(),
                text_elements: Vec::new(),
            }],
            source(),
            SpawnAgentOptions {
                parent_thread_id: Some(parent.thread_id),
                ..Default::default()
            },
        )
        .await
        .expect("spawn delegated child");
    let collaboration_thread = manager
        .get_thread(collaboration.thread_id)
        .await
        .expect("collaboration child should be registered");
    let collaboration_events = wait_for_turn_end(&collaboration_thread).await;
    assert_successful_turn(&collaboration_events, "collaboration child canonical turn");

    let fork = parent
        .thread
        .session
        .services
        .agent_control
        .spawn_agent_with_metadata(
            config,
            vec![UserInput::Text {
                text: "answer as a full-history child".to_string(),
                text_elements: Vec::new(),
            }],
            source(),
            SpawnAgentOptions {
                fork_parent_spawn_call_id: Some("call-full-history".to_string()),
                fork_mode: Some(SpawnAgentForkMode::FullHistory),
                parent_thread_id: Some(parent.thread_id),
                ..Default::default()
            },
        )
        .await
        .expect("fork delegated child with full history");
    let fork_thread = manager
        .get_thread(fork.thread_id)
        .await
        .expect("full-history child should be registered");
    let fork_events = wait_for_turn_end(&fork_thread).await;
    assert_successful_turn(&fork_events, "full-history child canonical turn");
    assert!(
        raw_history_items(&fork_thread.session.clone_history().await)
            .into_iter()
            .any(|item| matches!(
                item,
            ResponseItem::Message {
                role,
                phase: Some(MessagePhase::FinalAnswer),
                content,
                ..
                } if role == "assistant" && content.iter().any(|part| matches!(
                    part,
                    ContentItem::OutputText { text } if text == "first turn answer"
                ))
            ))
    );

    assert_eq!(parent_backend.child_session_factory_calls(), 2);
    let child_states = parent_backend.child_sessions();
    assert_eq!(child_states.len(), 2);
    assert!(!Arc::ptr_eq(&child_states[0], &parent_backend.state));
    assert!(!Arc::ptr_eq(&child_states[1], &parent_backend.state));
    assert!(!Arc::ptr_eq(&child_states[0], &child_states[1]));
    for child_state in child_states {
        let child_state = child_state.lock().expect("child backend state");
        assert_eq!(child_state.turn_count, 1);
        assert_eq!(child_state.requests.len(), 1);
    }
    assert_eq!(parent_backend.turn_count(), 1);
    assert_eq!(parent_backend.requests().len(), 2);

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn provider_mismatch_and_unsupported_prompt_fail_before_backend_stream() {
    let mismatch_backend = Arc::new(FakeBackend::new(Some("different-provider")));
    let mismatch_error = stream_error_for_backend(mismatch_backend.clone()).await;
    assert!(matches!(
        mismatch_error.details(),
        CodexErrorDetails::InvalidRequest(message) if message.contains("does not match")
    ));
    assert!(mismatch_backend.requests().is_empty());

    let matched_backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let unsupported_error = stream_error_for_backend(matched_backend.clone()).await;
    assert!(matches!(
        unsupported_error.details(),
        CodexErrorDetails::InvalidRequest(message) if message.contains("unsupported request data")
    ));
    assert!(matched_backend.requests().is_empty());
}

#[tokio::test]
async fn canonical_eof_is_non_retryable_and_codex_eof_keeps_legacy_error() {
    let (tx_event, rx_event) = tokio::sync::mpsc::channel(1);
    drop(tx_event);
    let codex_stream = SamplingModelStream::Codex(ResponseStream {
        rx_event,
        consumer_dropped: tokio_util::sync::CancellationToken::new(),
    });
    assert!(matches!(
        codex_stream.closed_stream_error().details(),
        CodexErrorDetails::Stream(message) if message == "stream closed before response.completed"
    ));
    let canonical_stream = SamplingModelStream::Canonical(Box::pin(FakeEventSource {
        events: VecDeque::new(),
    }));
    let canonical_eof_error = canonical_stream.closed_stream_error();
    assert!(matches!(
        canonical_eof_error.details(),
        CodexErrorDetails::InvalidRequest(message) if message.contains("closed before Completed")
    ));
    assert!(!canonical_eof_error.is_retryable());

    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_eof_test_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::closing_without_completion());
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");

    submit_user_turn(&started.thread, "the backend closes early").await;
    let events = wait_for_turn_end(&started.thread).await;
    let error_messages = events
        .iter()
        .filter_map(|event| match event {
            EventMsg::Error(error) => Some(error.message.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        error_messages
            .iter()
            .any(|message| message.contains("canonical model event stream closed before Completed")),
        "expected canonical EOF error, got: {error_messages:?}"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        EventMsg::TurnComplete(completed) if completed.error.is_some()
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventMsg::TurnAborted(_)))
    );
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(backend.requests().len(), 1, "InvalidRequest must not retry");

    started
        .thread
        .shutdown_and_wait()
        .await
        .expect("shutdown test thread");
}

#[test]
fn backend_errors_map_to_existing_retry_and_request_classes() {
    assert!(matches!(
        model_backend_error_to_codex(ModelBackendError::Failed {
            message: "temporary".to_string(),
            retryable: true,
        })
        .details(),
        CodexErrorDetails::Stream(message) if message == "temporary"
    ));
    assert!(matches!(
        model_backend_error_to_codex(ModelBackendError::Failed {
            message: "permanent".to_string(),
            retryable: false,
        })
        .details(),
        CodexErrorDetails::InvalidRequest(message) if message == "permanent"
    ));
    assert!(matches!(
        model_backend_error_to_codex(ModelBackendError::UnsupportedRequest(
            "shape".to_string()
        ))
        .details(),
        CodexErrorDetails::InvalidRequest(message) if message.contains("shape")
    ));
    assert!(matches!(
        model_backend_error_to_codex(ModelBackendError::Cancelled).details(),
        CodexErrorDetails::TurnAborted
    ));
}
