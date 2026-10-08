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
use codex_model_provider::RemoteCompactionSupport;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
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
    scripted_streams: VecDeque<Vec<Result<ModelEvent, ModelBackendError>>>,
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

    fn queue_stream(&self, events: Vec<Result<ModelEvent, ModelBackendError>>) {
        self.state
            .lock()
            .expect("backend state")
            .scripted_streams
            .push_back(events);
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
        let scripted_events = {
            let mut state = self.state.lock().expect("backend state");
            state.requests.push(RecordedRequest {
                turn_index: self.turn_index,
                model_id: model_id.to_string(),
                request: request.clone(),
            });
            state.scripted_streams.pop_front()
        };
        let events = scripted_events.unwrap_or_else(|| {
            let fallback = if self.close_without_completion {
                vec![ModelEvent::Started]
            } else if self.simple_answer {
                child_answer_events(self.turn_index, request_index)
            } else {
                events_for_request(self.turn_index, request_index)
            };
            fallback.into_iter().map(Ok).collect()
        });
        Box::pin(async move {
            let source = FakeEventSource {
                events: events.into_iter().collect(),
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

async fn canonical_local_compaction_test_config(
    codex_home: &std::path::Path,
) -> crate::config::Config {
    canonical_backend_test_config(codex_home).await
}

fn compact_summary_events(text: &str) -> Vec<Result<ModelEvent, ModelBackendError>> {
    let id = ModelItemId("canonical-compaction-summary".to_string());
    vec![
        Ok(ModelEvent::Started),
        Ok(ModelEvent::ReasoningSectionStarted {
            item_id: ModelItemId("ignored-content-section".to_string()),
            kind: tachyon_model::ModelReasoningDeltaKind::Content,
            section_index: 0,
        }),
        Ok(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::Message {
                id: id.clone(),
                phase: Some(ModelMessagePhase::Final),
            },
        )),
        Ok(ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
            id,
            phase: Some(ModelMessagePhase::Final),
            content: vec![ModelContent::Text(text.to_string())],
        })),
        Ok(completion(
            /*input_tokens*/ 7,
            /*total_tokens*/ 9,
            Some(true),
        )),
    ]
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
async fn injected_canonical_manual_compaction_uses_completed_events_and_a_fresh_runtime() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let mut config = canonical_local_compaction_test_config(codex_home.path()).await;
    config
        .features
        .disable(Feature::RemoteCompactionV2)
        .expect("disable remote compaction V2 for manual canonical compaction");
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    let provider = {
        started
            .thread
            .session
            .state
            .lock()
            .await
            .session_configuration
            .provider
            .clone()
    };
    assert!(provider.info().is_openai());
    assert_eq!(
        provider.capabilities().remote_compaction,
        RemoteCompactionSupport::V2
    );

    submit_user_turn(&started.thread, "seed history for local compaction").await;
    let seed_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&seed_events, "seed canonical turn");

    backend.queue_stream(compact_summary_events("canonical compaction summary"));
    started
        .thread
        .submit(Op::Compact)
        .await
        .expect("submit local compact");
    let compact_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&compact_events, "canonical manual compaction");
    assert!(
        !compact_events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    assert!(compact_events.iter().any(|event| matches!(
        event,
        EventMsg::TokenCount(TokenCountEvent {
            info: Some(info),
            ..
        }) if info.last_token_usage.input_tokens == 7
            && info.last_token_usage.total_tokens == 9
    )));
    let compacted_history = raw_history_items(&started.thread.session.clone_history().await);
    assert!(compacted_history.iter().any(|item| matches!(
        item,
        ResponseItem::Message { role, content, .. }
            if role == "user" && content.iter().any(|content| matches!(
                content,
                ContentItem::InputText { text }
                    if text.contains("seed history for local compaction")
            ))
    )));
    assert!(compacted_history.iter().any(|item| matches!(
        item,
        ResponseItem::Message { content, .. }
            if content.iter().any(|content| match content {
                ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                    text.contains("canonical compaction summary")
                }
                _ => false,
            })
    )));
    assert!(!compacted_history.iter().any(|item| matches!(
        item,
        ResponseItem::FunctionCall { call_id, .. } if call_id == "call-plan"
    )));
    assert!(!compacted_history.iter().any(|item| matches!(
        item,
        ResponseItem::FunctionCallOutput { call_id: Some(call_id), .. }
            if call_id == "call-plan"
    )));
    assert!(!compacted_history.iter().any(|item| matches!(
        item,
        ResponseItem::Message { role, content, .. }
            if role == "assistant" && content.iter().any(|content| matches!(
                content,
                ContentItem::OutputText { text } if text == "first turn answer"
            ))
    )));

    submit_user_turn(&started.thread, "continue after local compaction").await;
    let after_compaction_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&after_compaction_events, "turn after canonical compaction");

    let requests = backend.requests();
    assert_eq!(
        backend.turn_count(),
        3,
        "manual compaction gets its own handle"
    );
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].turn_index, 0);
    assert_eq!(requests[1].turn_index, 0);
    assert_eq!(
        requests[2].turn_index, 1,
        "compaction has a dedicated handle"
    );
    assert_eq!(
        requests[3].turn_index, 2,
        "the next user turn gets a fresh handle"
    );
    assert!(requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("seed history for local compaction")
            ))
    )));
    assert!(requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::ToolCall(call) if call.call_id.0 == "call-plan"
    )));
    assert!(requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::ToolResult(result) if result.call_id.0 == "call-plan"
    )));
    assert!(requests[3].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("canonical compaction summary")
            ))
    )));
    assert!(requests[3].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("seed history for local compaction")
            ))
    )));
    assert!(!requests[3].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::ToolCall(call) if call.call_id.0 == "call-plan"
    )));
    assert!(!requests[3].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::ToolResult(result) if result.call_id.0 == "call-plan"
    )));

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn canonical_compaction_eof_is_non_retryable_and_unsupported_history_fails_closed() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_local_compaction_test_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::closing_without_completion());
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    started
        .thread
        .submit(Op::Compact)
        .await
        .expect("submit local compact");
    let eof_events = wait_for_turn_end(&started.thread).await;
    assert!(eof_events.iter().any(|event| matches!(
        event,
        EventMsg::Error(error)
            if error.message.contains("canonical model event stream closed before Completed")
    )));
    assert!(
        !eof_events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(backend.requests().len(), 1, "canonical EOF is not retried");
    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;

    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_local_compaction_test_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    let seed_context = started
        .thread
        .session
        .new_turn_with_default_settings("unsupported-history-seed".to_string(), Default::default())
        .await;
    started
        .thread
        .session
        .record_conversation_items(
            seed_context.as_ref(),
            &[ResponseItem::AgentMessage {
                id: None,
                author: "worker".to_string(),
                recipient: "root".to_string(),
                content: vec![AgentMessageInputContent::InputText {
                    text: "provider-specific agent history".to_string(),
                }],
                internal_chat_message_metadata_passthrough: None,
            }],
        )
        .await;
    started
        .thread
        .submit(Op::Compact)
        .await
        .expect("submit local compact");
    let unsupported_events = wait_for_turn_end(&started.thread).await;
    assert!(unsupported_events.iter().any(|event| matches!(
        event,
        EventMsg::Error(error) if error.message.contains("unsupported request data")
    )));
    assert!(
        !unsupported_events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    assert_eq!(backend.turn_count(), 1);
    assert!(
        backend.requests().is_empty(),
        "unrepresentable canonical compaction must fail before streaming"
    );
    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn canonical_compaction_retries_retryable_failure_on_the_same_turn_handle() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_local_compaction_test_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    backend.queue_stream(vec![Err(ModelBackendError::Failed {
        message: "temporary compact failure".to_string(),
        retryable: true,
    })]);
    backend.queue_stream(compact_summary_events("retried compaction summary"));
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    started
        .thread
        .submit(Op::Compact)
        .await
        .expect("submit local compact");
    let events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&events, "retryable canonical compaction");
    let requests = backend.requests();
    assert_eq!(backend.turn_count(), 1, "retry keeps one turn runtime");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].turn_index, requests[1].turn_index);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn canonical_compaction_does_not_retry_non_retryable_backend_failure() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = canonical_local_compaction_test_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    backend.queue_stream(vec![Err(ModelBackendError::Failed {
        message: "permanent compact failure".to_string(),
        retryable: false,
    })]);
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    started
        .thread
        .submit(Op::Compact)
        .await
        .expect("submit local compact");
    let events = wait_for_turn_end(&started.thread).await;
    assert!(events.iter().any(|event| matches!(
        event,
        EventMsg::Error(error) if error.message.contains("permanent compact failure")
    )));
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(backend.requests().len(), 1);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
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

#[tokio::test]
async fn injected_canonical_auto_compaction_uses_local_runtime_with_openai_remote_v2_support() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let mut config = canonical_local_compaction_test_config(codex_home.path()).await;
    config.model_auto_compact_token_limit = Some(200_000);
    config
        .features
        .enable(Feature::RemoteCompactionV2)
        .expect("enable remote compaction V2 for canonical auto compaction");
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    let answer_id = ModelItemId("canonical-auto-seed-answer".to_string());
    backend.queue_stream(vec![
        Ok(ModelEvent::Started),
        Ok(ModelEvent::OutputItemStarted(
            ModelOutputItemStart::Message {
                id: answer_id.clone(),
                phase: Some(ModelMessagePhase::Final),
            },
        )),
        Ok(ModelEvent::OutputItemCompleted(ModelOutputItem::Message {
            id: answer_id,
            phase: Some(ModelMessagePhase::Final),
            content: vec![ModelContent::Text(
                "first auto-compaction answer".to_string(),
            )],
        })),
        Ok(completion(
            /*input_tokens*/ 250_000,
            /*total_tokens*/ 250_002,
            Some(true),
        )),
    ]);
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend.clone()));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    let provider = {
        started
            .thread
            .session
            .state
            .lock()
            .await
            .session_configuration
            .provider
            .clone()
    };
    assert!(provider.info().is_openai());
    assert_eq!(
        provider.capabilities().remote_compaction,
        RemoteCompactionSupport::V2
    );

    submit_user_turn(&started.thread, "seed auto-compaction history").await;
    let seed_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(&seed_events, "canonical auto-compaction seed turn");

    backend.queue_stream(compact_summary_events("canonical auto-compaction summary"));
    submit_user_turn(&started.thread, "continue after auto compaction").await;
    let compact_and_follow_up_events = wait_for_turn_end(&started.thread).await;
    assert_successful_turn(
        &compact_and_follow_up_events,
        "canonical auto compaction and follow-up turn",
    );
    assert!(
        compact_and_follow_up_events
            .iter()
            .any(|event| matches!(event, EventMsg::ContextCompacted(_)))
    );
    assert!(
        !compact_and_follow_up_events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    assert!(compact_and_follow_up_events.iter().any(|event| matches!(
        event,
        EventMsg::TokenCount(TokenCountEvent {
            info: Some(info),
            ..
        }) if info.last_token_usage.input_tokens == 7
            && info.last_token_usage.total_tokens == 9
    )));

    let requests = backend.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text)
                    if text.contains(crate::compact::SUMMARIZATION_PROMPT)
            ))
    )));
    assert!(requests[1].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("seed auto-compaction history")
            ))
    )));
    assert!(requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("canonical auto-compaction summary")
            ))
    )));
    assert!(requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("continue after auto compaction")
            ))
    )));
    assert!(!requests[2].request.input.iter().any(|item| matches!(
        item,
        ModelInputItem::Message(message)
            if message.content.iter().any(|content| matches!(
                content,
                ModelContent::Text(text) if text.contains("first auto-compaction answer")
            ))
    )));

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
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
