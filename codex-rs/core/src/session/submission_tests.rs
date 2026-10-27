use super::SessionSubmission;
use crate::session::SessionIo;
use codex_history::InputSource;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::Submission;
use codex_protocol::protocol::W3cTraceContext;
use codex_protocol::turn_input::TurnInput;
use codex_protocol::turn_input::TurnInputMode;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnInputSubmission;
use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::watch;

fn traced_user_input(text: &str, trace: W3cTraceContext) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }])
    .with_trace(Some(trace))
}

fn user_input(text: &str) -> TurnInput {
    TurnInput::UserInput {
        content: vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }],
        client_id: None,
    }
}

fn trace_context() -> W3cTraceContext {
    W3cTraceContext {
        traceparent: Some("00-1234567890abcdef1234567890abcdef-1234567890abcdef-01".to_string()),
        tracestate: Some("vendor=state".to_string()),
    }
}

#[test]
fn generic_submission_defaults_to_unknown_source() {
    let envelope = SessionSubmission::from(Submission {
        id: "submission-1".to_string(),
        op: Op::Interrupt,
        trace: None,
        parent_turn_id: None,
        root_turn_id: None,
    });

    assert_eq!(envelope.input_source, InputSource::Unknown);
    assert_eq!(envelope.submission.id, "submission-1");
    assert!(matches!(envelope.submission.op, Op::Interrupt));
}

#[tokio::test]
async fn turn_input_submission_uses_unknown_or_explicit_synthetic_source() {
    let (tx_sub, rx_sub) = async_channel::bounded(1);
    let (_tx_event, rx_event) = async_channel::unbounded();
    let io = Arc::new(SessionIo {
        tx_sub,
        rx_event,
        agent_status: watch::channel(AgentStatus::PendingInit).1,
        session_loop_termination: super::super::completed_session_loop_termination(),
    });

    let generic_trace = trace_context();
    let generic_request_trace = generic_trace.clone();
    let generic_io = Arc::clone(&io);
    let generic_submission = tokio::spawn(async move {
        generic_io
            .submit_turn_input(
                traced_user_input("generic input", generic_request_trace),
                TurnInputMode::StartIfIdle,
            )
            .await
    });
    let generic = rx_sub.recv().await.expect("generic submission");
    assert_eq!(generic.input_source, InputSource::Unknown);
    assert_eq!(generic.submission.trace, Some(generic_trace));
    let generic_id = generic.submission.id;
    let Op::TurnInput {
        request,
        mode,
        reply,
    } = generic.submission.op
    else {
        panic!("expected generic turn-input operation");
    };
    assert_eq!(mode, TurnInputMode::StartIfIdle);
    assert_eq!(request.input, user_input("generic input"));
    assert!(request.trace.is_none());
    reply
        .send(Ok(TurnInputSubmission::Started {
            turn_id: "generic-turn".to_string(),
        }))
        .expect("generic reply receiver should remain open");
    assert_eq!(
        generic_submission
            .await
            .expect("generic task should finish")
            .expect("generic submission should succeed"),
        TurnInputSubmission::Started {
            turn_id: "generic-turn".to_string(),
        }
    );
    assert!(!generic_id.is_empty());

    let synthetic_trace = trace_context();
    let synthetic_request_trace = synthetic_trace.clone();
    let synthetic_io = Arc::clone(&io);
    let synthetic_submission = tokio::spawn(async move {
        synthetic_io
            .submit_synthetic_turn_input(
                traced_user_input("synthetic input", synthetic_request_trace),
                TurnInputMode::StartIfIdle,
            )
            .await
    });
    let synthetic = rx_sub.recv().await.expect("synthetic submission");
    assert_eq!(synthetic.input_source, InputSource::Synthetic);
    assert_eq!(synthetic.submission.trace, Some(synthetic_trace));
    let Op::TurnInput {
        request,
        mode,
        reply,
    } = synthetic.submission.op
    else {
        panic!("expected synthetic turn-input operation");
    };
    assert_eq!(mode, TurnInputMode::StartIfIdle);
    assert_eq!(request.input, user_input("synthetic input"));
    assert!(request.trace.is_none());
    reply
        .send(Ok(TurnInputSubmission::Started {
            turn_id: "synthetic-turn".to_string(),
        }))
        .expect("synthetic reply receiver should remain open");
    assert_eq!(
        synthetic_submission
            .await
            .expect("synthetic task should finish")
            .expect("synthetic submission should succeed"),
        TurnInputSubmission::Started {
            turn_id: "synthetic-turn".to_string(),
        }
    );
}
