use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;

use super::ModelBackend;
use super::ModelBackendError;
use super::ModelBackendFuture;
use super::ModelCompactionItem;
use super::ModelCompactionResult;
use super::ModelEventSource;
use super::ModelEventStream;
use super::ModelTurnBackend;
use crate::ModelCompletion;
use crate::ModelContent;
use crate::ModelEvent;
use crate::ModelInputItem;
use crate::ModelItemId;
use crate::ModelMessage;
use crate::ModelMessagePhase;
use crate::ModelMessageRole;
use crate::ModelRequest;
use crate::ModelUsage;
use crate::route::ModelProtocol;
use crate::route::ModelProviderId;
use crate::route::ModelRoute;
use crate::route::ModelTransport;

#[derive(Debug, Clone, PartialEq)]
struct RecordedCall {
    turn_id: usize,
    provider_id: ModelProviderId,
    model_id: String,
    request: ModelRequest,
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedCompactionCall {
    turn_id: usize,
    route: ModelRoute,
    model_id: String,
    request: ModelRequest,
}

#[derive(Debug, Default)]
struct RecordingBackend {
    next_turn_id: AtomicUsize,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    compaction_calls: Arc<Mutex<Vec<RecordedCompactionCall>>>,
}

impl ModelBackend for RecordingBackend {
    fn supports_compaction(&self) -> bool {
        true
    }

    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        let turn_id = self.next_turn_id.fetch_add(1, Ordering::Relaxed);
        let route = ModelRoute::new(
            provider_id,
            ModelProtocol::new("test.protocol"),
            ModelTransport::Http,
        );
        Box::new(RecordingTurnBackend {
            turn_id,
            route,
            calls: Arc::clone(&self.calls),
            compaction_calls: Arc::clone(&self.compaction_calls),
        })
    }
}

#[derive(Debug)]
struct RecordingTurnBackend {
    turn_id: usize,
    route: ModelRoute,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    compaction_calls: Arc<Mutex<Vec<RecordedCompactionCall>>>,
}

impl ModelTurnBackend for RecordingTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn stream<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>> {
        self.calls
            .lock()
            .expect("recording backend lock poisoned")
            .push(RecordedCall {
                turn_id: self.turn_id,
                provider_id: self.route.provider_id().clone(),
                model_id: model_id.to_string(),
                request: request.clone(),
            });
        Box::pin(std::future::ready(Ok(
            Box::pin(FakeEventSource::default()) as ModelEventStream
        )))
    }

    fn compact<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelCompactionResult, ModelBackendError>> {
        self.compaction_calls
            .lock()
            .expect("recording backend lock poisoned")
            .push(RecordedCompactionCall {
                turn_id: self.turn_id,
                route: self.route.clone(),
                model_id: model_id.to_string(),
                request: request.clone(),
            });
        let mut history = (0..request.input.len())
            .map(|index| ModelCompactionItem::RetainedInput { index })
            .collect::<Vec<_>>();
        history.push(ModelCompactionItem::NewItem(ModelInputItem::Message(
            ModelMessage {
                role: ModelMessageRole::Assistant,
                phase: Some(ModelMessagePhase::Final),
                content: vec![ModelContent::Text("compacted summary".to_string())],
            },
        )));
        Box::pin(std::future::ready(Ok(ModelCompactionResult {
            history,
            usage: Some(ModelUsage {
                input_tokens: 12,
                output_tokens: 3,
                cached_input_tokens: Some(2),
                cache_write_input_tokens: None,
                reasoning_output_tokens: Some(1),
                total_tokens: Some(15),
            }),
        })))
    }
}

#[derive(Debug)]
struct DefaultCapabilityBackend {
    stream_calls: Arc<AtomicUsize>,
}

impl ModelBackend for DefaultCapabilityBackend {
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        Box::new(DefaultCapabilityTurnBackend {
            route: ModelRoute::new(
                provider_id,
                ModelProtocol::new("test.protocol"),
                ModelTransport::Http,
            ),
            stream_calls: Arc::clone(&self.stream_calls),
        })
    }
}

#[derive(Debug)]
struct DefaultCapabilityTurnBackend {
    route: ModelRoute,
    stream_calls: Arc<AtomicUsize>,
}

impl ModelTurnBackend for DefaultCapabilityTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn stream<'a>(
        &'a mut self,
        _model_id: &'a str,
        _request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>> {
        self.stream_calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(std::future::ready(Ok(
            Box::pin(FakeEventSource::default()) as ModelEventStream
        )))
    }
}

#[derive(Debug)]
struct FakeEventSource {
    events: VecDeque<Result<ModelEvent, ModelBackendError>>,
}

impl Default for FakeEventSource {
    fn default() -> Self {
        Self {
            events: VecDeque::from([
                Ok(ModelEvent::Started),
                Ok(ModelEvent::TextDelta {
                    item_id: ModelItemId("message-1".to_string()),
                    delta: "hello".to_string(),
                }),
                Ok(ModelEvent::Completed(ModelCompletion {
                    usage: None,
                    end_turn: Some(true),
                })),
            ]),
        }
    }
}

impl ModelEventSource for FakeEventSource {
    fn poll_next(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<ModelEvent, ModelBackendError>>> {
        Poll::Ready(self.get_mut().events.pop_front())
    }
}

fn poll_ready<T>(mut future: ModelBackendFuture<'_, T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("fake backend future unexpectedly pending"),
    }
}

fn collect_events(mut stream: ModelEventStream) -> Vec<ModelEvent> {
    let mut context = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    loop {
        match stream.as_mut().poll_next(&mut context) {
            Poll::Ready(Some(Ok(event))) => events.push(event),
            Poll::Ready(Some(Err(error))) => panic!("fake event stream failed: {error}"),
            Poll::Ready(None) => return events,
            Poll::Pending => panic!("fake event stream unexpectedly pending"),
        }
    }
}

fn user_request(text: &str) -> ModelRequest {
    ModelRequest {
        instructions: "Answer precisely.".to_string(),
        input: vec![ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![ModelContent::Text(text.to_string())],
        })],
        ..ModelRequest::default()
    }
}

#[test]
fn factory_creates_provider_bound_turns_and_preserves_canonical_streams() {
    let backend = RecordingBackend::default();
    assert_eq!(poll_ready(backend.prepare_session()), Ok(()));

    let provider_a = ModelProviderId::new("provider-a");
    let provider_b = ModelProviderId::new("provider-b");
    let mut first_turn = backend.begin_turn(provider_a.clone());
    let mut second_turn = backend.begin_turn(provider_b.clone());

    assert_eq!(first_turn.route().provider_id(), &provider_a);
    assert_eq!(second_turn.route().provider_id(), &provider_b);
    assert_eq!(first_turn.try_recover_after_stream_error(), None);

    let first_request = user_request("What is 2 + 2?");
    let follow_up_request = user_request("Explain that answer briefly.");
    let second_provider_request = user_request("Say hello.");
    let first_events = collect_events(
        poll_ready(first_turn.stream("selected-model", &first_request)).expect("first stream"),
    );
    let follow_up_events = collect_events(
        poll_ready(first_turn.stream("selected-model", &follow_up_request))
            .expect("follow-up stream"),
    );
    let second_events = collect_events(
        poll_ready(second_turn.stream("other-model", &second_provider_request))
            .expect("second provider stream"),
    );

    let expected_events = vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            item_id: ModelItemId("message-1".to_string()),
            delta: "hello".to_string(),
        },
        ModelEvent::Completed(ModelCompletion {
            usage: None,
            end_turn: Some(true),
        }),
    ];
    assert_eq!(first_events, expected_events);
    assert_eq!(follow_up_events, expected_events);
    assert_eq!(second_events, expected_events);

    let calls = backend
        .calls
        .lock()
        .expect("recording backend lock poisoned")
        .clone();
    assert_eq!(
        calls,
        vec![
            RecordedCall {
                turn_id: 0,
                provider_id: provider_a.clone(),
                model_id: "selected-model".to_string(),
                request: first_request,
            },
            RecordedCall {
                turn_id: 0,
                provider_id: provider_a,
                model_id: "selected-model".to_string(),
                request: follow_up_request,
            },
            RecordedCall {
                turn_id: 1,
                provider_id: provider_b,
                model_id: "other-model".to_string(),
                request: second_provider_request,
            },
        ]
    );
}

#[test]
fn compaction_is_unsupported_by_default_without_streaming() {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let backend = DefaultCapabilityBackend {
        stream_calls: Arc::clone(&stream_calls),
    };
    assert!(!backend.supports_compaction());
    let mut turn = backend.begin_turn(ModelProviderId::new("provider-a"));

    let error = poll_ready(turn.compact("selected-model", &user_request("compact this")))
        .expect_err("the default turn compaction method must be unsupported");

    assert_eq!(
        error,
        ModelBackendError::UnsupportedRequest(
            "backend-managed compaction is not supported by this backend".to_string()
        )
    );
    assert_eq!(stream_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn opted_in_compaction_uses_provider_bound_turn_and_preserves_result() {
    let backend = RecordingBackend::default();
    assert!(backend.supports_compaction());

    let provider_a = ModelProviderId::new("provider-a");
    let provider_b = ModelProviderId::new("provider-b");
    let request = user_request("compact this conversation");
    let mut first_turn = backend.begin_turn(provider_a.clone());
    let result = poll_ready(first_turn.compact("selected-model", &request))
        .expect("opted-in backend compaction");
    let first_events = collect_events(
        poll_ready(first_turn.stream("selected-model", &request)).expect("same-turn sampling"),
    );
    let mut second_turn = backend.begin_turn(provider_b.clone());
    let second_events = collect_events(
        poll_ready(second_turn.stream("other-model", &request)).expect("fresh-turn sampling"),
    );

    assert_eq!(
        result,
        ModelCompactionResult {
            history: vec![
                ModelCompactionItem::RetainedInput { index: 0 },
                ModelCompactionItem::NewItem(ModelInputItem::Message(ModelMessage {
                    role: ModelMessageRole::Assistant,
                    phase: Some(ModelMessagePhase::Final),
                    content: vec![ModelContent::Text("compacted summary".to_string())],
                })),
            ],
            usage: Some(ModelUsage {
                input_tokens: 12,
                output_tokens: 3,
                cached_input_tokens: Some(2),
                cache_write_input_tokens: None,
                reasoning_output_tokens: Some(1),
                total_tokens: Some(15),
            }),
        }
    );
    assert_eq!(first_events.len(), 3);
    assert_eq!(second_events.len(), 3);

    let compaction_calls = backend
        .compaction_calls
        .lock()
        .expect("recording backend lock poisoned")
        .clone();
    assert_eq!(
        compaction_calls,
        vec![RecordedCompactionCall {
            turn_id: 0,
            route: ModelRoute::new(
                provider_a.clone(),
                ModelProtocol::new("test.protocol"),
                ModelTransport::Http,
            ),
            model_id: "selected-model".to_string(),
            request: request.clone(),
        }]
    );
    let stream_calls = backend
        .calls
        .lock()
        .expect("recording backend lock poisoned")
        .clone();
    assert_eq!(
        stream_calls,
        vec![
            RecordedCall {
                turn_id: 0,
                provider_id: provider_a,
                model_id: "selected-model".to_string(),
                request: request.clone(),
            },
            RecordedCall {
                turn_id: 1,
                provider_id: provider_b,
                model_id: "other-model".to_string(),
                request,
            },
        ]
    );
}

#[test]
fn compaction_result_validates_only_unique_in_range_retained_inputs() {
    let valid = ModelCompactionResult {
        history: vec![
            ModelCompactionItem::RetainedInput { index: 1 },
            ModelCompactionItem::NewItem(user_request("summary").input[0].clone()),
        ],
        usage: Some(ModelUsage {
            input_tokens: 12,
            output_tokens: 3,
            cached_input_tokens: Some(2),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(1),
            total_tokens: Some(15),
        }),
    };
    let unchanged = valid.clone();
    valid
        .validate_retained_inputs(2)
        .expect("last input index is valid");
    assert_eq!(valid, unchanged);

    let new_item_without_inputs = ModelCompactionResult {
        history: vec![ModelCompactionItem::NewItem(
            user_request("summary").input[0].clone(),
        )],
        usage: None,
    };
    new_item_without_inputs
        .validate_retained_inputs(0)
        .expect("new items do not require a retained source");
    ModelCompactionResult {
        history: Vec::new(),
        usage: None,
    }
    .validate_retained_inputs(0)
    .expect("reference-only validation permits empty history");

    for index in [0, usize::MAX] {
        let invalid = ModelCompactionResult {
            history: vec![ModelCompactionItem::RetainedInput { index }],
            usage: None,
        };
        let unchanged = invalid.clone();
        let error = invalid
            .validate_retained_inputs(0)
            .expect_err("out-of-range retained inputs must fail");
        assert!(matches!(
            error,
            ModelBackendError::Failed {
                retryable: false,
                message,
            } if message.contains(&index.to_string()) && message.contains("input length is 0")
        ));
        assert_eq!(invalid, unchanged);
    }

    let duplicate = ModelCompactionResult {
        history: vec![
            ModelCompactionItem::RetainedInput { index: 0 },
            ModelCompactionItem::NewItem(user_request("summary").input[0].clone()),
            ModelCompactionItem::RetainedInput { index: 0 },
        ],
        usage: None,
    };
    let unchanged = duplicate.clone();
    let error = duplicate
        .validate_retained_inputs(1)
        .expect_err("duplicate retained references must fail");
    assert!(matches!(
        error,
        ModelBackendError::Failed {
            retryable: false,
            message,
        } if message.contains("input index 0 more than once")
    ));
    assert_eq!(duplicate, unchanged);
}

#[test]
fn child_session_factory_is_unsupported_by_default() {
    let backend = RecordingBackend::default();
    let error = poll_ready(backend.new_child_session())
        .expect_err("a backend must explicitly implement child-session creation");

    assert_eq!(
        error,
        ModelBackendError::UnsupportedRequest(
            "independent child sessions are not supported by this backend".to_string()
        )
    );
}
