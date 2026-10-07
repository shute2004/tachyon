use std::sync::Arc;
use std::time::Duration;

use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use crate::tasks::SessionTask;
use crate::tasks::SessionTaskResult;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::turn_input::NotSubmittedReason;
use codex_protocol::turn_input::TurnInput as SubmittedTurnInput;
use codex_protocol::turn_input::TurnInputMode;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnInputSubmission;
use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::Session;
use super::handle;
use super::handle_recovery;

struct HoldUntilCancelledTask;

impl SessionTask for HoldUntilCancelledTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    fn span_name(&self) -> &'static str {
        "session_task.turn_input_admission_test"
    }

    async fn run(
        self: Arc<Self>,
        _session: Arc<Session>,
        _context: Arc<TurnContext>,
        _input: Vec<super::TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        cancellation_token.cancelled().await;
        Ok(None)
    }
}

#[tokio::test]
async fn handle_realtime_and_recovery_wait_without_mutating_state_then_admit_in_order() {
    let (session, turn_context, _events) = make_session_and_context_with_rx().await;
    let active_turn_id = turn_context.sub_id.clone();
    hold_active_regular_task(&session, turn_context).await;
    let original_settings = session.thread_settings_snapshot().await;
    let admission = session.input_queue.acquire_admission().await;

    let mut normal = Box::pin(handle(
        &session,
        TurnInputRequest::user_input(vec![UserInput::Text {
            text: "normal first".to_string(),
            text_elements: Vec::new(),
        }])
        .with_thread_settings(ThreadSettingsOverrides {
            approval_policy: Some(AskForApproval::Never),
            ..Default::default()
        }),
        TurnInputMode::StartOrSteer,
        "normal-submission".to_string(),
    ));
    let mut realtime = Box::pin(session.route_realtime_text_input("realtime second".to_string()));
    let mut recovery = Box::pin(handle_recovery(
        &session,
        ThreadSettingsOverrides {
            approval_policy: Some(AskForApproval::OnRequest),
            ..Default::default()
        },
        Default::default(),
        "recovery-third".to_string(),
    ));

    assert!(futures::poll!(normal.as_mut()).is_pending());
    assert!(futures::poll!(realtime.as_mut()).is_pending());
    assert!(futures::poll!(recovery.as_mut()).is_pending());
    assert_eq!(session.thread_settings_snapshot().await, original_settings);
    assert!(
        session
            .input_queue
            .get_pending_input(&session.active_turn)
            .await
            .0
            .is_empty()
    );

    drop(admission);
    assert!(futures::poll!(realtime.as_mut()).is_pending());
    assert!(futures::poll!(recovery.as_mut()).is_pending());
    assert_eq!(
        timeout(Duration::from_secs(5), normal)
            .await
            .expect("normal admission should acquire the released gate")
            .expect("normal admission"),
        TurnInputSubmission::Steered {
            turn_id: active_turn_id,
        }
    );
    assert_eq!(
        session.thread_settings_snapshot().await.approval_policy,
        AskForApproval::Never
    );
    timeout(Duration::from_secs(5), realtime)
        .await
        .expect("realtime admission should proceed after the normal waiter");
    assert_eq!(
        timeout(Duration::from_secs(5), recovery)
            .await
            .expect("recovery admission should proceed after the earlier waiters")
            .expect("recovery admission"),
        TurnInputSubmission::NotSubmitted {
            reason: NotSubmittedReason::NotIdle,
        }
    );
    assert_eq!(
        session
            .input_queue
            .get_pending_input(&session.active_turn)
            .await
            .0,
        vec![text_input("normal first"), text_input("realtime second")]
    );
    assert_eq!(
        session.thread_settings_snapshot().await.approval_policy,
        AskForApproval::Never
    );
    let available_after_rejection = timeout(
        Duration::from_secs(5),
        session.input_queue.acquire_admission(),
    )
    .await
    .expect("NotIdle rejection releases admission");
    drop(available_after_rejection);

    timeout(
        Duration::from_secs(5),
        session.abort_all_tasks(TurnAbortReason::Interrupted),
    )
    .await
    .expect("test task cancellation should finish");
}

#[tokio::test]
async fn cancelled_waiter_releases_its_slot_and_admission_is_session_local() {
    let (first_session, first_context, _first_events) = make_session_and_context_with_rx().await;
    let first_turn_id = first_context.sub_id.clone();
    hold_active_regular_task(&first_session, first_context).await;
    let (second_session, second_context, _second_events) = make_session_and_context_with_rx().await;
    let second_turn_id = second_context.sub_id.clone();
    hold_active_regular_task(&second_session, second_context).await;

    let original_settings = first_session.thread_settings_snapshot().await;
    let first_admission = first_session.input_queue.acquire_admission().await;
    let mut cancelled = Box::pin(handle(
        &first_session,
        user_request("cancelled waiter"),
        TurnInputMode::StartOrSteer,
        "cancelled-submission".to_string(),
    ));
    assert!(futures::poll!(cancelled.as_mut()).is_pending());
    drop(cancelled);
    assert_eq!(
        first_session.thread_settings_snapshot().await,
        original_settings
    );
    assert!(
        first_session
            .input_queue
            .get_pending_input(&first_session.active_turn)
            .await
            .0
            .is_empty()
    );

    let mut independent = Box::pin(handle(
        &second_session,
        user_request("other session"),
        TurnInputMode::StartOrSteer,
        "other-session-submission".to_string(),
    ));
    let independent_result = timeout(Duration::from_secs(5), independent.as_mut())
        .await
        .expect("second Session does not share the first Session gate")
        .expect("independent steer");
    assert_eq!(
        independent_result,
        TurnInputSubmission::Steered {
            turn_id: second_turn_id,
        }
    );
    assert_eq!(
        second_session
            .input_queue
            .get_pending_input(&second_session.active_turn)
            .await
            .0,
        vec![text_input("other session")]
    );

    drop(first_admission);
    let invalid = timeout(
        Duration::from_secs(5),
        handle(
            &first_session,
            TurnInputRequest::new(SubmittedTurnInput::ResponseItem(ResponseItem::Other)),
            TurnInputMode::StartOrSteer,
            "invalid-submission".to_string(),
        ),
    )
    .await
    .expect("invalid submission should return instead of holding admission")
    .expect_err("non-user start-or-steer input is an error");
    assert!(matches!(
        invalid.details(),
        CodexErrorDetails::InvalidRequest(message)
            if message == "only user input can steer a turn"
    ));
    assert!(
        first_session
            .input_queue
            .get_pending_input(&first_session.active_turn)
            .await
            .0
            .is_empty(),
        "invalid input must leave the pending queue unchanged"
    );
    let later = timeout(
        Duration::from_secs(5),
        handle(
            &first_session,
            user_request("after cancellation"),
            TurnInputMode::StartOrSteer,
            "after-cancel-submission".to_string(),
        ),
    )
    .await
    .expect("later submission should not hang if an earlier input failed")
    .expect("later waiter should acquire the released slot");
    assert_eq!(
        later,
        TurnInputSubmission::Steered {
            turn_id: first_turn_id,
        }
    );
    assert_eq!(
        first_session
            .input_queue
            .get_pending_input(&first_session.active_turn)
            .await
            .0,
        vec![text_input("after cancellation")]
    );
    let available_after_error = timeout(
        Duration::from_secs(5),
        first_session.input_queue.acquire_admission(),
    )
    .await
    .expect("input error releases admission");
    drop(available_after_error);

    timeout(
        Duration::from_secs(5),
        first_session.abort_all_tasks(TurnAbortReason::Interrupted),
    )
    .await
    .expect("first test task cancellation should finish");
    timeout(
        Duration::from_secs(5),
        second_session.abort_all_tasks(TurnAbortReason::Interrupted),
    )
    .await
    .expect("second test task cancellation should finish");
}

async fn hold_active_regular_task(session: &Arc<Session>, turn_context: Arc<TurnContext>) {
    session
        .spawn_task(
            Arc::clone(&turn_context),
            Vec::new(),
            HoldUntilCancelledTask,
        )
        .await;
}

fn user_request(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }])
}

fn text_input(text: &str) -> super::TurnInput {
    super::TurnInput::UserInput {
        content: vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }],
        client_id: None,
        input_association: None,
    }
}
