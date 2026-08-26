use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;
use tracing::info;
use tracing::instrument;
use tracing::trace_span;
use tracing::warn;

use crate::guardian::routes_approval_to_guardian;
use crate::model_runtime::ModelTurnRuntime;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::session::INITIAL_SUBMIT_ID;
use crate::session::session::Session;
use crate::session::turn::build_prompt;
use codex_features::Feature;
use codex_otel::STARTUP_PREWARM_AGE_AT_FIRST_TURN_METRIC;
use codex_otel::STARTUP_PREWARM_DURATION_METRIC;
use codex_otel::SessionTelemetry;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::BaseInstructions;

/// Session-owned background preparation that may yield a prepared model turn runtime.
pub(crate) struct SessionStartupPreparationHandle {
    task: AbortOnDropHandle<CodexResult<ModelTurnRuntime>>,
    started_at: Instant,
    timeout: Duration,
}

pub(crate) enum SessionStartupPreparationResolution {
    Cancelled,
    Ready(Box<ModelTurnRuntime>),
    Unavailable {
        status: &'static str,
        preparation_duration: Option<Duration>,
    },
}

impl SessionStartupPreparationHandle {
    pub(crate) fn new(
        task: JoinHandle<CodexResult<ModelTurnRuntime>>,
        started_at: Instant,
        timeout: Duration,
    ) -> Self {
        Self {
            task: AbortOnDropHandle::new(task),
            started_at,
            timeout,
        }
    }

    pub(crate) async fn abort(self) {
        self.task.abort();
        let _ = self.task.await;
    }

    // Keep the existing telemetry span name while the metrics contract still uses prewarm naming.
    #[instrument(name = "startup_prewarm.resolve", level = "trace", skip_all)]
    async fn resolve(
        self,
        session_telemetry: &SessionTelemetry,
        cancellation_token: &CancellationToken,
    ) -> SessionStartupPreparationResolution {
        let resolve_started_at = Instant::now();
        let Self {
            mut task,
            started_at,
            timeout,
        } = self;
        let age_at_first_turn = started_at.elapsed();
        let remaining = timeout.saturating_sub(age_at_first_turn);

        let resolution = if task.is_finished() {
            Self::resolution_from_join_result(task.await, started_at)
        } else {
            match tokio::select! {
                _ = cancellation_token.cancelled() => None,
                result = tokio::time::timeout(remaining, &mut task) => Some(result),
            } {
                Some(Ok(result)) => Self::resolution_from_join_result(result, started_at),
                Some(Err(_elapsed)) => {
                    task.abort();
                    info!("startup model preparation timed out before the first turn could use it");
                    SessionStartupPreparationResolution::Unavailable {
                        status: "timed_out",
                        preparation_duration: Some(started_at.elapsed()),
                    }
                }
                None => {
                    task.abort();
                    session_telemetry.record_startup_phase(
                        "startup_prewarm_resolve",
                        resolve_started_at.elapsed(),
                        Some("cancelled"),
                    );
                    session_telemetry.record_duration(
                        STARTUP_PREWARM_AGE_AT_FIRST_TURN_METRIC,
                        age_at_first_turn,
                        &[("status", "cancelled")],
                    );
                    session_telemetry.record_duration(
                        STARTUP_PREWARM_DURATION_METRIC,
                        started_at.elapsed(),
                        &[("status", "cancelled")],
                    );
                    return SessionStartupPreparationResolution::Cancelled;
                }
            }
        };
        let status = match &resolution {
            SessionStartupPreparationResolution::Cancelled => "cancelled",
            SessionStartupPreparationResolution::Ready(_) => "ready",
            SessionStartupPreparationResolution::Unavailable { status, .. } => status,
        };
        session_telemetry.record_startup_phase(
            "startup_prewarm_resolve",
            resolve_started_at.elapsed(),
            Some(status),
        );

        match resolution {
            SessionStartupPreparationResolution::Cancelled => {
                SessionStartupPreparationResolution::Cancelled
            }
            SessionStartupPreparationResolution::Ready(prepared_turn_runtime) => {
                session_telemetry.record_duration(
                    STARTUP_PREWARM_AGE_AT_FIRST_TURN_METRIC,
                    age_at_first_turn,
                    &[("status", "consumed")],
                );
                SessionStartupPreparationResolution::Ready(prepared_turn_runtime)
            }
            SessionStartupPreparationResolution::Unavailable {
                status,
                preparation_duration,
            } => {
                session_telemetry.record_duration(
                    STARTUP_PREWARM_AGE_AT_FIRST_TURN_METRIC,
                    age_at_first_turn,
                    &[("status", status)],
                );
                if let Some(preparation_duration) = preparation_duration {
                    session_telemetry.record_duration(
                        STARTUP_PREWARM_DURATION_METRIC,
                        preparation_duration,
                        &[("status", status)],
                    );
                }
                SessionStartupPreparationResolution::Unavailable {
                    status,
                    preparation_duration,
                }
            }
        }
    }

    fn resolution_from_join_result(
        result: std::result::Result<CodexResult<ModelTurnRuntime>, tokio::task::JoinError>,
        started_at: Instant,
    ) -> SessionStartupPreparationResolution {
        match result {
            Ok(Ok(prepared_turn_runtime)) => {
                SessionStartupPreparationResolution::Ready(Box::new(prepared_turn_runtime))
            }
            Ok(Err(err)) => {
                warn!("startup model preparation failed: {err:#}");
                SessionStartupPreparationResolution::Unavailable {
                    status: "failed",
                    preparation_duration: None,
                }
            }
            Err(err) => {
                warn!("startup model preparation join failed: {err}");
                SessionStartupPreparationResolution::Unavailable {
                    status: "join_failed",
                    preparation_duration: Some(started_at.elapsed()),
                }
            }
        }
    }
}

impl Session {
    pub(crate) async fn schedule_startup_preparation(self: &Arc<Self>, base_instructions: String) {
        if self.features().enabled(Feature::CodeModePrewarm)
            && self.services.code_mode_service.is_available()
        {
            let session = Arc::clone(self);
            tokio::spawn(async move {
                if session.services.code_mode_service.session().await.is_err() {
                    warn!("code-mode host startup prewarm failed");
                }
            });
        }

        let model_runtime = self.services.model_runtime();
        if !model_runtime.startup_preparation_uses_turn_runtime() {
            // Some backends only need session-scoped preparation before the first request.
            tokio::spawn(async move {
                if let Err(err) = model_runtime.prepare_session().await {
                    warn!("startup model session preparation failed: {err:#}");
                }
            });
            return;
        }

        let session_telemetry = self.services.session_telemetry.clone();
        // Preserve the current Codex timeout source while the concrete preparation implementation
        // remains the Responses WebSocket adapter.
        let preparation_timeout = self.provider().await.websocket_connect_timeout();
        let started_at = Instant::now();
        let startup_preparation_session = Arc::clone(self);
        let startup_preparation = tokio::spawn(
            async move {
                let result = schedule_startup_preparation_inner(
                    startup_preparation_session,
                    base_instructions,
                )
                .await;
                let status = if result.is_ok() { "ready" } else { "failed" };
                session_telemetry.record_startup_phase(
                    "startup_prewarm_total",
                    started_at.elapsed(),
                    Some(status),
                );
                session_telemetry.record_duration(
                    STARTUP_PREWARM_DURATION_METRIC,
                    started_at.elapsed(),
                    &[("status", status)],
                );
                result
            }
            .instrument(trace_span!(
                "startup_prewarm",
                otel.name = "startup_prewarm",
                thread.id = %self.thread_id(),
            )),
        );
        // These state helpers retain their legacy names temporarily; the stored type is now the
        // generic preparation handle and the compatibility alias is removed when the large session
        // module is next touched for a semantic change.
        self.set_session_startup_prewarm(SessionStartupPreparationHandle::new(
            startup_preparation,
            started_at,
            preparation_timeout,
        ))
        .await;
    }

    pub(crate) async fn consume_startup_preparation_for_regular_turn(
        &self,
        cancellation_token: &CancellationToken,
    ) -> SessionStartupPreparationResolution {
        let Some(startup_preparation) = self.take_session_startup_prewarm().await else {
            return SessionStartupPreparationResolution::Unavailable {
                status: "not_scheduled",
                preparation_duration: None,
            };
        };
        startup_preparation
            .resolve(&self.services.session_telemetry, cancellation_token)
            .await
    }

    /// Compatibility entry point for initialization code that has not yet been renamed.
    pub(crate) async fn schedule_startup_prewarm(self: &Arc<Self>, base_instructions: String) {
        self.schedule_startup_preparation(base_instructions).await;
    }
}

async fn schedule_startup_preparation_inner(
    session: Arc<Session>,
    base_instructions: String,
) -> CodexResult<ModelTurnRuntime> {
    let preparation_started_at = Instant::now();
    let startup_turn_context = session
        .new_startup_prewarm_turn_with_sub_id(INITIAL_SUBMIT_ID.to_owned())
        .await;
    startup_turn_context.session_telemetry.record_startup_phase(
        "startup_prewarm_create_turn_context",
        preparation_started_at.elapsed(),
        /*status*/ None,
    );
    if routes_approval_to_guardian(&startup_turn_context) {
        let guardian_session = Arc::clone(&session);
        let guardian_parent_turn = Arc::clone(&startup_turn_context);
        drop(tokio::spawn(async move {
            if let Err(err) = guardian_session
                .guardian_review_session
                .initialize(Arc::clone(&guardian_session), guardian_parent_turn)
                .await
            {
                warn!("failed to initialize guardian review session: {err:#}");
            }
        }));
    }
    let startup_cancellation_token = CancellationToken::new();
    let built_tools_started_at = Instant::now();
    // Startup preparation runs before run_turn and needs its own tool-building snapshot.
    let step_context = session
        .capture_step_context(
            Arc::clone(&startup_turn_context),
            &startup_cancellation_token,
        )
        .await?;
    startup_turn_context.session_telemetry.record_startup_phase(
        "startup_prewarm_build_tools",
        built_tools_started_at.elapsed(),
        /*status*/ None,
    );
    let build_prompt_started_at = Instant::now();
    let startup_prompt = build_prompt(
        Vec::new(),
        step_context.as_ref(),
        BaseInstructions {
            text: base_instructions,
            provenance: None,
        },
    );
    startup_turn_context.session_telemetry.record_startup_phase(
        "startup_prewarm_build_prompt",
        build_prompt_started_at.elapsed(),
        /*status*/ None,
    );
    let window_id = session.current_window_id().await;
    let responses_metadata = startup_turn_context
        .turn_metadata_state
        .to_responses_metadata(
            session.installation_id.clone(),
            window_id,
            CodexResponsesRequestKind::Prewarm,
        );
    let mut turn_runtime = session.services.model_runtime().begin_turn();
    let backend_preparation_started_at = Instant::now();
    turn_runtime
        .prepare(
            &startup_prompt,
            &step_context.settings.model_info,
            &step_context.session_telemetry,
            step_context.settings.reasoning_effort().cloned(),
            step_context.settings.reasoning_summary,
            step_context.settings.service_tier.clone(),
            &responses_metadata,
        )
        .await?;
    startup_turn_context.session_telemetry.record_startup_phase(
        "startup_prewarm_websocket_warmup",
        backend_preparation_started_at.elapsed(),
        /*status*/ None,
    );
    Ok(turn_runtime)
}
