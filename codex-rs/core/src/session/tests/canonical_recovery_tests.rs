use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_login::CodexAuth;
use codex_protocol::protocol::EventMsg;
use tachyon_model::ModelEvent;
use tachyon_model::ModelRequest;
use tachyon_model::backend::ModelBackend;
use tachyon_model::backend::ModelBackendError;
use tachyon_model::backend::ModelBackendFuture;
use tachyon_model::backend::ModelEventStream;
use tachyon_model::backend::ModelTurnBackend;
use tachyon_model::route::ModelProviderId;
use tachyon_model::route::ModelRoute;

use super::FakeBackend;
use crate::model_runtime::ModelRuntime;
use crate::thread_manager::NewThread;
use crate::thread_manager::StartThreadOptions;
use crate::thread_manager::ThreadManager;

#[derive(Clone, Debug)]
struct RecoveringBackend {
    inner: FakeBackend,
    mode: RecoveryMode,
    recovery_calls: Arc<AtomicUsize>,
}

#[derive(Clone, Debug)]
enum RecoveryMode {
    OncePerTurn {
        warning: &'static str,
    },
    ChangeProvider {
        warning: &'static str,
        provider_id: &'static str,
    },
}

impl RecoveringBackend {
    fn recover_once_per_turn(warning: &'static str) -> Self {
        Self {
            inner: FakeBackend::new(/*route_provider_override*/ None),
            mode: RecoveryMode::OncePerTurn { warning },
            recovery_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn recover_to_different_provider(warning: &'static str, provider_id: &'static str) -> Self {
        Self {
            inner: FakeBackend::new(/*route_provider_override*/ None),
            mode: RecoveryMode::ChangeProvider {
                warning,
                provider_id,
            },
            recovery_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn queue_stream(&self, events: Vec<Result<ModelEvent, ModelBackendError>>) {
        self.inner.queue_stream(events);
    }

    fn requests(&self) -> Vec<super::RecordedRequest> {
        self.inner.requests()
    }

    fn turn_count(&self) -> usize {
        self.inner.turn_count()
    }

    fn recovery_calls(&self) -> usize {
        self.recovery_calls.load(Ordering::Relaxed)
    }
}

impl ModelBackend for RecoveringBackend {
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        let inner = self.inner.begin_turn(provider_id);
        Box::new(RecoveringTurnBackend {
            route: inner.route().clone(),
            inner,
            mode: self.mode.clone(),
            recovered: false,
            recovery_calls: Arc::clone(&self.recovery_calls),
        })
    }
}

struct RecoveringTurnBackend {
    route: ModelRoute,
    inner: Box<dyn ModelTurnBackend>,
    mode: RecoveryMode,
    recovered: bool,
    recovery_calls: Arc<AtomicUsize>,
}

impl ModelTurnBackend for RecoveringTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn try_recover_after_stream_error(&mut self) -> Option<String> {
        self.recovery_calls.fetch_add(1, Ordering::Relaxed);
        if self.recovered {
            return None;
        }
        self.recovered = true;

        match &self.mode {
            RecoveryMode::OncePerTurn { warning } => Some(warning.to_string()),
            RecoveryMode::ChangeProvider {
                warning,
                provider_id,
            } => {
                self.route = ModelRoute::new(
                    ModelProviderId::new(*provider_id),
                    self.route.protocol().clone(),
                    self.route.transport(),
                );
                Some(warning.to_string())
            }
        }
    }

    fn stream<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>> {
        self.inner.stream(model_id, request)
    }
}

async fn zero_retry_config(codex_home: &std::path::Path) -> crate::config::Config {
    let mut config = super::canonical_backend_test_config(codex_home).await;
    config.model_provider.stream_max_retries = Some(0);
    config
}

fn retryable_error(message: &str) -> Vec<Result<ModelEvent, ModelBackendError>> {
    vec![Err(ModelBackendError::Failed {
        message: message.to_string(),
        retryable: true,
    })]
}

fn successful_answer_events(
    turn_index: usize,
    request_index: usize,
) -> Vec<Result<ModelEvent, ModelBackendError>> {
    super::events_for_request(turn_index, request_index)
        .into_iter()
        .map(Ok)
        .collect()
}

fn assert_recovery_warning(events: &[EventMsg], warning: &str, original_error: &str) {
    assert!(
        events.iter().any(|event| matches!(
            event,
            EventMsg::Warning(warning_event)
                if warning_event.message.contains(warning)
                    && warning_event.message.contains(original_error)
        )),
        "missing recovery warning `{warning}` with `{original_error}` in {events:?}"
    );
}

fn assert_turn_error(events: &[EventMsg], message: &str) {
    assert!(
        events.iter().any(|event| match event {
            EventMsg::Error(error) => error.message.contains(message),
            EventMsg::TurnComplete(completion) => completion
                .error
                .as_ref()
                .is_some_and(|error| error.message.contains(message)),
            _ => false,
        }),
        "missing turn error `{message}` in {events:?}"
    );
}

async fn start_thread(
    config: crate::config::Config,
    backend: Arc<dyn ModelBackend>,
) -> (ThreadManager, NewThread) {
    let options = StartThreadOptions::new(config.clone())
        .with_model_runtime(ModelRuntime::from_backend(backend));
    let manager = ThreadManager::with_models_provider_for_tests(
        CodexAuth::from_api_key("dummy"),
        config.model_provider.clone(),
    );
    let started = manager.start_thread(options).await.expect("start thread");
    (manager, started)
}

#[tokio::test]
async fn canonical_recovery_retries_same_handle_and_resets_for_next_turn() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = zero_retry_config(codex_home.path()).await;
    let backend = Arc::new(RecoveringBackend::recover_once_per_turn(
        "canonical backend recovered request path",
    ));
    backend.queue_stream(retryable_error("first recoverable sampling failure"));
    backend.queue_stream(successful_answer_events(
        /*turn_index*/ 0, /*request_index*/ 1,
    ));
    backend.queue_stream(retryable_error("second recoverable sampling failure"));
    backend.queue_stream(successful_answer_events(
        /*turn_index*/ 1, /*request_index*/ 1,
    ));
    let (manager, started) = start_thread(config, backend.clone()).await;

    super::submit_user_turn(&started.thread, "recover first turn").await;
    let first_events = super::wait_for_turn_end(&started.thread).await;
    super::assert_successful_turn(&first_events, "first recovered canonical turn");
    assert_recovery_warning(
        &first_events,
        "canonical backend recovered request path",
        "first recoverable sampling failure",
    );

    super::submit_user_turn(&started.thread, "recover second turn").await;
    let second_events = super::wait_for_turn_end(&started.thread).await;
    super::assert_successful_turn(&second_events, "second recovered canonical turn");
    assert_recovery_warning(
        &second_events,
        "canonical backend recovered request path",
        "second recoverable sampling failure",
    );

    let requests = backend.requests();
    assert_eq!(backend.recovery_calls(), 2);
    assert_eq!(backend.turn_count(), 2);
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].turn_index, 0);
    assert_eq!(requests[1].turn_index, 0);
    assert_eq!(requests[2].turn_index, 1);
    assert_eq!(requests[3].turn_index, 1);
    assert_eq!(requests[0].model_id, requests[1].model_id);
    assert_eq!(requests[2].model_id, requests[3].model_id);
    assert_eq!(requests[0].request, requests[1].request);
    assert_eq!(requests[2].request, requests[3].request);

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn default_canonical_recovery_none_exhausts_original_error_without_fallback() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = zero_retry_config(codex_home.path()).await;
    let backend = Arc::new(FakeBackend::new(/*route_provider_override*/ None));
    backend.queue_stream(retryable_error("unrecovered sampling failure"));
    let (manager, started) = start_thread(config, backend.clone()).await;

    super::submit_user_turn(&started.thread, "fail without recovery").await;
    let events = super::wait_for_turn_end(&started.thread).await;
    assert_turn_error(&events, "unrecovered sampling failure");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EventMsg::RawResponseCompleted(_)))
    );
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(backend.requests().len(), 1);

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn nonretryable_canonical_failure_never_calls_recovery() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = zero_retry_config(codex_home.path()).await;
    let backend = Arc::new(RecoveringBackend::recover_once_per_turn(
        "should not recover permanent failure",
    ));
    backend.queue_stream(vec![Err(ModelBackendError::Failed {
        message: "permanent sampling failure".to_string(),
        retryable: false,
    })]);
    let (manager, started) = start_thread(config, backend.clone()).await;

    super::submit_user_turn(&started.thread, "permanent failure").await;
    let events = super::wait_for_turn_end(&started.thread).await;
    assert_turn_error(&events, "permanent sampling failure");
    assert_eq!(backend.recovery_calls(), 0);
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(backend.requests().len(), 1);

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}

#[tokio::test]
async fn provider_changing_recovery_fails_closed_before_next_backend_stream() {
    let codex_home = tempfile::tempdir().expect("temporary Codex home");
    let config = zero_retry_config(codex_home.path()).await;
    let backend = Arc::new(RecoveringBackend::recover_to_different_provider(
        "canonical backend changed provider",
        "different-provider",
    ));
    backend.queue_stream(retryable_error("provider-changing sampling failure"));
    backend.queue_stream(successful_answer_events(
        /*turn_index*/ 0, /*request_index*/ 1,
    ));
    let (manager, started) = start_thread(config, backend.clone()).await;

    super::submit_user_turn(&started.thread, "provider changing recovery").await;
    let events = super::wait_for_turn_end(&started.thread).await;
    assert_recovery_warning(
        &events,
        "canonical backend changed provider",
        "provider-changing sampling failure",
    );
    assert_turn_error(&events, "does not match");
    assert_eq!(backend.recovery_calls(), 1);
    assert_eq!(backend.turn_count(), 1);
    assert_eq!(
        backend.requests().len(),
        1,
        "provider mutation must fail before the retry streams again"
    );

    manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
}
