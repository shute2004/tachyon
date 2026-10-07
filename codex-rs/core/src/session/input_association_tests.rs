use std::sync::Arc;

use codex_history::InputAssociation;
use codex_history::InputIdentity;
use codex_history::InputSource;
use codex_history::InputStreamIncarnation;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::turn_input::NotSubmittedReason;
use codex_protocol::turn_input::TurnInput as SubmittedTurnInput;
use codex_protocol::turn_input::TurnInputMode;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnInputSubmission;
use codex_protocol::user_input::ByteRange;
use codex_protocol::user_input::TextElement;
use codex_protocol::user_input::UserInput;
use codex_thread_store::PersistContext;
use pretty_assertions::assert_eq;
use test_case::test_case;
use tokio_util::sync::CancellationToken;

use crate::rollout::recorder::RolloutRecorder;
use crate::session::tests::attach_in_memory_thread_store;
use crate::session::tests::attach_thread_persistence;
use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use crate::tasks::SessionTask;
use crate::tasks::SessionTaskResult;

use super::super::TurnInput;

struct HoldUntilCancelledTask;

impl SessionTask for HoldUntilCancelledTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    fn span_name(&self) -> &'static str {
        "session_task.input_association_test"
    }

    async fn run(
        self: Arc<Self>,
        _session: Arc<super::super::session::Session>,
        _turn_context: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        cancellation_token.cancelled().await;
        Ok(None)
    }
}

fn user_input(text: &str) -> Vec<UserInput> {
    vec![UserInput::Text {
        text: text.to_string(),
        text_elements: vec![TextElement::new(
            ByteRange {
                start: 0,
                end: text.len(),
            },
            Some("input marker".to_string()),
        )],
    }]
}

async fn hold_active_turn(
    session: &Arc<super::super::session::Session>,
    context: Arc<TurnContext>,
) {
    session
        .spawn_task(context, Vec::new(), HoldUntilCancelledTask)
        .await;
}

async fn steer(
    session: &Arc<super::super::session::Session>,
    context: &TurnContext,
    content: Vec<UserInput>,
    expected_turn_id: &str,
    source: InputSource,
) -> TurnInputSubmission {
    super::super::turn_input::handle_with_source(
        session,
        TurnInputRequest::new(SubmittedTurnInput::UserInput {
            content,
            client_id: Some("client-message".to_string()),
        }),
        TurnInputMode::Steer {
            expected_turn_id: expected_turn_id.to_string(),
        },
        format!("submission-{}", context.sub_id),
        source,
    )
    .await
    .expect("steer request should be valid")
}

async fn assert_accepted_without_association(
    session: Arc<super::super::session::Session>,
    context: Arc<TurnContext>,
    text: &str,
    source: InputSource,
) {
    hold_active_turn(&session, Arc::clone(&context)).await;
    assert!(matches!(
        steer(
            &session,
            &context,
            user_input(text),
            &context.sub_id,
            source
        )
        .await,
        TurnInputSubmission::Steered { .. }
    ));
    let (pending, _) = session
        .input_queue
        .get_pending_input(&session.active_turn)
        .await;
    assert!(matches!(
        pending.as_slice(),
        [TurnInput::UserInput {
            input_association: None,
            ..
        }]
    ));
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}

#[test]
fn queued_association_is_process_local_and_preserves_legacy_bytes() {
    let identity = InputIdentity {
        thread_id: ThreadId::new(),
        incarnation: InputStreamIncarnation::new(),
        sequence: std::num::NonZeroU64::new(1).expect("one is nonzero"),
    };
    let expected = r#"{"UserInput":{"content":[{"type":"text","text":"legacy prompt","text_elements":[]}],"client_id":"client-message"}}"#;
    let content = vec![UserInput::Text {
        text: "legacy prompt".to_string(),
        text_elements: Vec::new(),
    }];
    let queued = TurnInput::UserInput {
        content: content.clone(),
        client_id: Some("client-message".to_string()),
        input_association: Some(InputAssociation {
            identity,
            source: InputSource::Unknown,
        }),
    };
    assert_eq!(serde_json::to_string(&queued).unwrap(), expected);
    assert!(matches!(
        serde_json::from_str::<TurnInput>(expected).unwrap(),
        TurnInput::UserInput {
            input_association: None,
            ..
        }
    ));
    let mut forged: serde_json::Value = serde_json::from_str(expected).unwrap();
    forged["UserInput"]["input_association"] = serde_json::json!({
        "identity": { "thread_id": identity.thread_id, "incarnation": identity.incarnation, "sequence": 1 },
        "source": "unknown"
    });
    assert!(matches!(
        serde_json::from_value::<TurnInput>(forged).unwrap(),
        TurnInput::UserInput {
            input_association: None,
            ..
        }
    ));
}

#[test_case(InputSource::Unknown; "unknown")]
#[test_case(InputSource::Synthetic; "synthetic")]
#[tokio::test]
async fn accepted_steer_reserves_after_rejections_and_records_only_original_prompt(
    source: InputSource,
) {
    let (mut session, context, events) = make_session_and_context_with_rx().await;
    let path = attach_thread_persistence(Arc::get_mut(&mut session).unwrap()).await;
    hold_active_turn(&session, Arc::clone(&context)).await;

    let rejected = steer(
        &session,
        &context,
        user_input("wrong turn"),
        "other-turn",
        source,
    )
    .await;
    assert!(matches!(
        rejected,
        TurnInputSubmission::NotSubmitted {
            reason: NotSubmittedReason::ExpectedTurnMismatch { .. }
        }
    ));
    assert_eq!(
        steer(&session, &context, Vec::new(), &context.sub_id, source).await,
        TurnInputSubmission::NotSubmitted {
            reason: NotSubmittedReason::EmptyInput
        }
    );
    assert_eq!(
        super::super::turn_input::handle_recovery(
            &session,
            Default::default(),
            Default::default(),
            "recovery-while-active".to_string(),
        )
        .await
        .unwrap(),
        TurnInputSubmission::NotSubmitted {
            reason: NotSubmittedReason::NotIdle
        }
    );

    let input = user_input("accepted marker input");
    assert!(matches!(
        steer(&session, &context, input.clone(), &context.sub_id, source).await,
        TurnInputSubmission::Steered { .. }
    ));
    let (pending, _) = session
        .input_queue
        .get_pending_input(&session.active_turn)
        .await;
    let TurnInput::UserInput {
        content,
        client_id,
        input_association: Some(association),
    } = pending.into_iter().next().unwrap()
    else {
        panic!("accepted input should carry its reservation");
    };
    assert_eq!(content, input);
    assert_eq!(client_id.as_deref(), Some("client-message"));
    assert_eq!(association.identity.thread_id, session.thread_id);
    assert_eq!(association.identity.sequence.get(), 1);
    assert_eq!(association.source, source);

    crate::hook_runtime::record_pending_input(
        &session,
        &context,
        TurnInput::UserInput {
            content: content.clone(),
            client_id,
            input_association: Some(association),
        },
        vec!["unassociated hook context".to_string()],
        PersistContext::Standard,
    )
    .await;
    session.flush_rollout().await.unwrap();
    let completed = loop {
        let event = events.try_recv().expect("user item event should be queued");
        if let EventMsg::ItemCompleted(completed) = event.msg
            && let TurnItem::UserMessage(message) = completed.item
            && message.client_id.as_deref() == Some("client-message")
        {
            break message;
        }
    };
    assert_eq!(completed.content, input);

    let (items, thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&path).await.unwrap();
    assert_eq!((thread_id, parse_errors), (Some(session.thread_id), 0));
    let response_items = items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => Some(envelope),
            _ => None,
        })
        .collect::<Vec<&ResponseItemEnvelope>>();
    let associated = response_items
        .iter()
        .filter_map(|envelope| envelope.metadata.as_ref()?.input_association)
        .collect::<Vec<_>>();
    assert_eq!(associated, vec![association]);
    assert_eq!(
        response_items
            .iter()
            .filter(|envelope| {
                envelope
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.input_association)
                    == Some(association)
                    && matches!(&envelope.item, ResponseItem::Message { role, content, .. }
                    if role == "user" && content.iter().any(|item| matches!(
                        item, ContentItem::InputText { text } if text == "accepted marker input"
                    )))
            })
            .count(),
        1
    );
    assert!(response_items.iter().any(|envelope| {
        envelope
            .metadata
            .as_ref()
            .is_none_or(|metadata| metadata.input_association.is_none())
            && matches!(&envelope.item, ResponseItem::Message { content, .. }
            if content.iter().any(|item| matches!(
                item, ContentItem::InputText { text } if text.contains("unassociated hook context")
            )))
    }));
    assert!(!response_items.iter().any(|envelope| matches!(
        &envelope.item, ResponseItem::Message { role, content, .. }
            if role == "user" && content.is_empty()
    )));
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}

#[tokio::test]
async fn reservation_errors_keep_accepted_steer_unassociated() {
    let (mut unsupported, unsupported_context, _) = make_session_and_context_with_rx().await;
    attach_in_memory_thread_store(Arc::get_mut(&mut unsupported).unwrap()).await;
    assert_accepted_without_association(
        unsupported,
        unsupported_context,
        "unsupported",
        InputSource::Synthetic,
    )
    .await;

    let (missing, missing_context, _) = make_session_and_context_with_rx().await;
    assert_accepted_without_association(
        missing,
        missing_context,
        "missing live thread",
        InputSource::Unknown,
    )
    .await;
}
