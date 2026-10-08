use codex_core::NotSubmittedReason;
use codex_core::RecoverTurnRequest;
use codex_core::StartIfIdleSubmission;
use codex_core::SteerSubmission;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_core::TurnInputSubmission;
use codex_core::TurnStartOptions;
use codex_core::config::Constrained;
use codex_history::InputSource;
use codex_history::RolloutItem;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::protocol::TurnEnvironmentSelections;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::submit_thread_settings;
use core_test_support::test_codex::local;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use test_case::test_case;
use tokio::sync::Barrier;
use tokio::sync::oneshot;
use tokio::time::timeout;

fn user_message_request(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }])
}

async fn submit_user_message(
    codex: &codex_core::CodexThread,
    text: &str,
) -> codex_protocol::error::Result<TurnInputSubmission> {
    codex.start_or_steer_turn(user_message_request(text)).await
}

#[test_case(ModeKind::Default, ModeKind::Plan; "automatic input cannot enter Plan")]
#[test_case(ModeKind::Plan, ModeKind::Default; "automatic input cannot leave Plan")]
#[tokio::test]
async fn start_turn_if_idle_keeps_automatic_plan_rejections_atomic(
    current_mode: ModeKind,
    proposed_mode: ModeKind,
) {
    let server = responses::start_mock_server().await;
    let test = test_codex()
        .build_with_auto_env(&server)
        .await
        .expect("build turn-input submission session");
    let mut collaboration_mode = test.codex.config_snapshot().await.collaboration_mode;
    collaboration_mode.mode = current_mode;
    submit_thread_settings(
        &test.codex,
        ThreadSettingsOverrides {
            collaboration_mode: Some(collaboration_mode.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("set the current collaboration mode");
    let current_settings = test.codex.thread_settings_snapshot().await;
    collaboration_mode.mode = proposed_mode;
    let overrides = ThreadSettingsOverrides {
        collaboration_mode: Some(collaboration_mode.clone()),
        ..Default::default()
    };
    let submission = test
        .codex
        .start_turn_if_idle(
            TurnInputRequest::new(TurnInput::ResponseItem(responses::user_message_item(
                "rejected automatic input",
            )))
            .with_thread_settings(overrides.clone()),
        )
        .await
        .expect("automatic Plan admission should return a typed rejection");
    assert_eq!(
        submission,
        StartIfIdleSubmission::NotSubmitted {
            reason: NotSubmittedReason::PlanMode,
        }
    );
    assert_eq!(
        test.codex.thread_settings_snapshot().await,
        current_settings
    );

    // Rejection releases the idle reservation, and an explicit user can make
    // either transition without receiving the rejected automatic input.
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let started = test
        .codex
        .start_turn_if_idle(
            user_message_request("explicit user input").with_thread_settings(overrides),
        )
        .await
        .expect("rejection must release the idle reservation for explicit user input");
    assert!(matches!(started, StartIfIdleSubmission::Started { .. }));
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(
        test.codex.config_snapshot().await.collaboration_mode,
        collaboration_mode
    );
    let request = response_mock.single_request();
    assert!(request.body_contains_text("explicit user input"));
    assert!(!request.body_contains_text("rejected automatic input"));
}

#[tokio::test]
async fn recover_turn_if_idle_preserves_id_and_resumes_plan_mode() {
    let server = responses::start_mock_server().await;
    let response_mock =
        responses::mount_sse_once(&server, responses::sse_completed("resp-1")).await;
    let test = test_codex()
        .build_with_auto_env(&server)
        .await
        .expect("build recovered turn session");
    let turn_id = "durable-recovered-turn";

    let submission = test
        .codex
        .recover_turn_if_idle(RecoverTurnRequest {
            turn_id: turn_id.to_string(),
            thread_settings: ThreadSettingsOverrides {
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Plan,
                    settings: Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
            trace: None,
            cyber_access_program: None,
        })
        .await
        .expect("recovered turn should start");
    assert_eq!(
        submission,
        StartIfIdleSubmission::Started {
            turn_id: turn_id.to_string(),
        }
    );
    assert_eq!(
        test.codex.config_snapshot().await.collaboration_mode.mode,
        ModeKind::Plan
    );

    let started = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnStarted(_))
    })
    .await;
    let EventMsg::TurnStarted(started) = started else {
        unreachable!("wait_for_event returned unexpected event");
    };
    assert_eq!(started.turn_id, turn_id);
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = response_mock.single_request();
    let turn_metadata: Value = serde_json::from_str(
        request
            .header("x-codex-turn-metadata")
            .as_deref()
            .expect("recovered turn should include turn metadata"),
    )
    .expect("recovered turn metadata should be valid JSON");
    assert_eq!(turn_metadata["turn_trigger"].as_str(), Some("retry"));
    let user_input_groups = request.message_input_text_groups("user");
    assert_eq!(user_input_groups.len(), 1);
    assert_eq!(user_input_groups[0].len(), 1);
    assert!(user_input_groups[0][0].starts_with("<environment_context>"));
}

/// Concurrent submissions must start exactly one turn and steer the other message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_input_submission_reports_started_and_steered_for_concurrent_submissions() {
    let (release_response, response_gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![
            StreamingSseChunk {
                gate: None,
                body: responses::sse(vec![ev_response_created("resp-1")]),
            },
            StreamingSseChunk {
                gate: Some(response_gate),
                body: responses::sse(vec![ev_completed("resp-1")]),
            },
        ],
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        }],
    ])
    .await;
    let test = test_codex()
        .with_model("gpt-5.4")
        .build_with_streaming_server(&server)
        .await
        .expect("build turn-input submission session");
    let codex = Arc::clone(&test.codex);
    let barrier = Arc::new(Barrier::new(3));

    let first_submission = tokio::spawn({
        let codex = Arc::clone(&codex);
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            submit_user_message(codex.as_ref(), "first message").await
        }
    });
    let second_submission = tokio::spawn({
        let codex = Arc::clone(&codex);
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            submit_user_message(codex.as_ref(), "second message").await
        }
    });
    barrier.wait().await;

    timeout(
        Duration::from_secs(5),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("the started turn should reach its first model request");
    release_response
        .send(())
        .expect("response gate should remain open");

    let (first_submission, second_submission) = timeout(Duration::from_secs(5), async {
        tokio::join!(first_submission, second_submission)
    })
    .await
    .expect("both concurrent submissions should resolve once their messages are submitted");
    let first_submission = first_submission
        .expect("first submission task should finish")
        .expect("first user message should be submitted");
    let second_submission = second_submission
        .expect("second submission task should finish")
        .expect("second user message should be submitted");
    let (started_turn_id, steered_turn_id, started_message) =
        match (&first_submission, &second_submission) {
            (
                TurnInputSubmission::Started { turn_id: started },
                TurnInputSubmission::Steered { turn_id: steered },
            ) => (started, steered, "first message"),
            (
                TurnInputSubmission::Steered { turn_id: steered },
                TurnInputSubmission::Started { turn_id: started },
            ) => (started, steered, "second message"),
            _ => panic!(
                "concurrent messages must start exactly one turn and steer the other: \
             {first_submission:?}, {second_submission:?}"
            ),
        };
    assert_eq!(started_turn_id, steered_turn_id);

    wait_for_event(codex.as_ref(), |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    let request_bodies: Vec<Value> = requests
        .iter()
        .map(|request| serde_json::from_slice(request).expect("parse model request"))
        .collect();
    assert!(request_bodies[0].to_string().contains(started_message));
    assert!(request_bodies[1].to_string().contains("first message"));
    assert!(request_bodies[1].to_string().contains("second message"));

    server.shutdown().await;
}

#[test_case(false; "start_if_idle")]
#[test_case(true; "start_or_steer_idle")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_start_and_steer_persist_unknown_input_associations(
    use_start_or_steer: bool,
) -> anyhow::Result<()> {
    let (release_response, response_gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![
            StreamingSseChunk {
                gate: None,
                body: responses::sse(vec![ev_response_created("resp-input-association-1")]),
            },
            StreamingSseChunk {
                gate: Some(response_gate),
                body: responses::sse(vec![ev_completed("resp-input-association-1")]),
            },
        ],
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![
                ev_response_created("resp-input-association-2"),
                ev_completed("resp-input-association-2"),
            ]),
        }],
    ])
    .await;
    let test = test_codex()
        .with_model("gpt-5.4")
        .build_with_streaming_server(&server)
        .await?;

    let make_input = |text: &str| {
        TurnInputRequest::new(TurnInput::UserInput {
            content: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            client_id: Some("same-client-message-id".to_string()),
        })
    };
    let turn_id = if use_start_or_steer {
        match test
            .codex
            .start_or_steer_turn(make_input("identity start prompt"))
            .await?
        {
            TurnInputSubmission::Started { turn_id } => turn_id,
            other => panic!("nonempty user input should start a turn: {other:?}"),
        }
    } else {
        match test
            .codex
            .start_turn_if_idle(make_input("identity start prompt"))
            .await?
        {
            StartIfIdleSubmission::Started { turn_id } => turn_id,
            other => panic!("nonempty user input should start a turn: {other:?}"),
        }
    };
    timeout(
        Duration::from_secs(5),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("started turn should reach the first model request");

    assert_eq!(
        test.codex
            .steer_turn(make_input("identity steer prompt"), turn_id.clone())
            .await?,
        SteerSubmission::Steered { turn_id }
    );
    release_response
        .send(())
        .expect("first response gate should remain open");

    let raw_user_items = timeout(Duration::from_secs(10), async {
        let mut raw_user_items = Vec::new();
        while raw_user_items.len() < 2 {
            let event = test
                .codex
                .next_event()
                .await
                .expect("event stream should stay open");
            if let EventMsg::RawResponseItem(raw) = event.msg
                && user_prompt_text(&raw.item).is_some_and(|text| {
                    text == "identity start prompt" || text == "identity steer prompt"
                })
            {
                raw_user_items.push(raw.item);
            }
        }
        raw_user_items
    })
    .await
    .expect("both original raw inputs should be emitted");
    assert!(raw_user_items.iter().all(|item| {
        !serde_json::to_value(item)
            .expect("serialize public raw response item")
            .to_string()
            .contains("input_association")
    }));
    wait_for_event(test.codex.as_ref(), |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    test.codex.flush_rollout().await?;

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| { !String::from_utf8_lossy(request).contains("input_association") })
    );

    let rollout_path = test.codex.rollout_path().expect("rollout path");
    let (rollout, thread_id, parse_errors) =
        codex_core::RolloutRecorder::load_rollout_items(&rollout_path).await?;
    assert_eq!(parse_errors, 0);
    let rollout_thread_id = rollout.iter().find_map(|item| match item {
        RolloutItem::SessionMeta(meta) => Some(meta.meta.id),
        _ => None,
    });
    assert_eq!(thread_id, rollout_thread_id);
    let associations = rollout
        .iter()
        .filter_map(|item| match item {
            RolloutItem::ResponseItem(envelope) => {
                let text = user_prompt_text(&envelope.item)?;
                let association = envelope.metadata.as_ref()?.input_association?;
                Some((text.to_string(), association))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        associations
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["identity start prompt", "identity steer prompt"]
    );
    assert_eq!(associations.len(), 2);
    let [(_, started_association), (_, steered_association)] = associations.as_slice() else {
        panic!("the two accepted original prompts should both have associations");
    };
    assert_eq!(started_association.source, InputSource::Unknown);
    assert_eq!(steered_association.source, InputSource::Unknown);
    assert_eq!(
        started_association.identity.thread_id,
        thread_id.expect("rollout thread id")
    );
    assert_eq!(
        steered_association.identity.thread_id,
        started_association.identity.thread_id
    );
    assert_eq!(
        steered_association.identity.incarnation,
        started_association.identity.incarnation
    );
    assert_ne!(
        steered_association.identity.sequence,
        started_association.identity.sequence
    );
    assert_eq!(started_association.identity.sequence.get(), 1);
    assert_eq!(steered_association.identity.sequence.get(), 2);

    server.shutdown().await;
    Ok(())
}

fn user_prompt_text(item: &ResponseItem) -> Option<&str> {
    match item {
        ResponseItem::Message { role, content, .. } if role == "user" => {
            content.iter().find_map(|item| match item {
                ContentItem::InputText { text } => Some(text.as_str()),
                _ => None,
            })
        }
        _ => None,
    }
}

/// A user start and recovery admission share the same per-session decision gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_input_admission_serializes_user_start_and_recovery() {
    let (release_response, response_gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![vec![
        StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![ev_response_created("resp-admission-race")]),
        },
        StreamingSseChunk {
            gate: Some(response_gate),
            body: responses::sse(vec![ev_completed("resp-admission-race")]),
        },
    ]])
    .await;
    let test = test_codex()
        .with_model("gpt-5.4")
        .build_with_streaming_server(&server)
        .await
        .expect("build turn-input admission session");
    let codex = Arc::clone(&test.codex);
    let barrier = Arc::new(Barrier::new(3));

    let user_start = tokio::spawn({
        let codex = Arc::clone(&codex);
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            codex
                .start_turn_if_idle(user_message_request("racing original input"))
                .await
        }
    });
    let recovery = tokio::spawn({
        let codex = Arc::clone(&codex);
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            codex
                .recover_turn_if_idle(RecoverTurnRequest {
                    turn_id: "existing-recovery-id".to_string(),
                    thread_settings: ThreadSettingsOverrides::default(),
                    trace: None,
                    cyber_access_program: None,
                })
                .await
        }
    });
    barrier.wait().await;

    let (user_start, recovery) = timeout(Duration::from_secs(5), async {
        tokio::join!(user_start, recovery)
    })
    .await
    .expect("both typed admission receipts should resolve before model completion");
    let user_start = user_start
        .expect("user-start task should finish")
        .expect("user-start admission should return a typed result");
    let recovery = recovery
        .expect("recovery task should finish")
        .expect("recovery admission should return a typed result");

    timeout(
        Duration::from_secs(5),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("the winning admission should reach the mock model");
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let request: Value = serde_json::from_slice(&requests[0]).expect("parse model request");
    let request_text = request.to_string();
    match (&user_start, &recovery) {
        (
            StartIfIdleSubmission::Started { .. },
            StartIfIdleSubmission::NotSubmitted {
                reason: NotSubmittedReason::NotIdle,
            },
        ) => {
            assert_eq!(input_text_occurrences(&request, "racing original input"), 1);
        }
        (
            StartIfIdleSubmission::NotSubmitted {
                reason: NotSubmittedReason::NotIdle,
            },
            StartIfIdleSubmission::Started { turn_id },
        ) => {
            assert_eq!(turn_id, "existing-recovery-id");
            assert_eq!(input_text_occurrences(&request, "racing original input"), 0);
            assert!(request_text.contains("<environment_context>"));
        }
        _ => panic!(
            "user start and recovery must yield exactly one Started and one NotIdle: \
             {user_start:?}, {recovery:?}"
        ),
    }

    release_response
        .send(())
        .expect("response gate should remain open until both receipts are checked");
    timeout(
        Duration::from_secs(5),
        wait_for_event(codex.as_ref(), |event| {
            matches!(event, EventMsg::TurnComplete(_))
        }),
    )
    .await
    .expect("the started request should complete after releasing the mock gate");
    timeout(Duration::from_secs(5), server.shutdown())
        .await
        .expect("mock streaming server should shut down");
}

fn input_text_occurrences(request: &Value, expected: &str) -> usize {
    request["input"]
        .as_array()
        .expect("model request input array")
        .iter()
        .filter(|item| item["role"] == "user")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|item| item["text"].as_str() == Some(expected))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_input_submission_applies_thread_settings_only_after_accepted_input() {
    let (release_response, response_gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![
            StreamingSseChunk {
                gate: None,
                body: responses::sse(vec![ev_response_created("resp-1")]),
            },
            StreamingSseChunk {
                gate: Some(response_gate),
                body: responses::sse(vec![ev_completed("resp-1")]),
            },
        ],
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        }],
    ])
    .await;
    let test = test_codex()
        .with_model("gpt-5.4")
        .with_config(|config| {
            config.permissions.approval_policy = Constrained::allow_any(AskForApproval::OnRequest);
        })
        .build_with_streaming_server(&server)
        .await
        .expect("build approval-constrained turn-input submission session");
    let codex = &test.codex;

    let started = submit_user_message(codex, "start turn")
        .await
        .expect("first message should start a turn");
    let TurnInputSubmission::Started { turn_id } = started else {
        panic!("first message should start a turn");
    };
    timeout(
        Duration::from_secs(5),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("started turn should reach its first model request");

    let steered_cwd = test.config.cwd.join("steered-environment");
    let steered_environments =
        TurnEnvironmentSelections::new(steered_cwd.clone(), vec![local(steered_cwd)]);
    let steered = codex
        .start_or_steer_turn(
            user_message_request("steer active turn").with_thread_settings(
                ThreadSettingsOverrides {
                    approval_policy: Some(AskForApproval::Never),
                    environments: Some(steered_environments.clone()),
                    ..Default::default()
                },
            ),
        )
        .await
        .expect("persistent settings should not reject a steer");
    assert_eq!(steered, TurnInputSubmission::Steered { turn_id });
    assert_eq!(
        codex.config_snapshot().await.approval_policy,
        AskForApproval::Never
    );
    assert_eq!(
        codex.environment_selections().await,
        steered_environments.environments
    );

    release_response
        .send(())
        .expect("response gate should remain open");
    wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let rejected_cwd = test.config.cwd.join("rejected-environment");
    let rejected = codex
        .steer_turn(
            user_message_request("no active turn").with_thread_settings(ThreadSettingsOverrides {
                approval_policy: Some(AskForApproval::OnRequest),
                environments: Some(TurnEnvironmentSelections::new(
                    rejected_cwd.clone(),
                    vec![local(rejected_cwd)],
                )),
                ..Default::default()
            }),
            "missing-turn".to_string(),
        )
        .await
        .expect("idle steer should return a typed rejection");
    assert_eq!(
        rejected,
        SteerSubmission::NotSubmitted {
            reason: NotSubmittedReason::NoActiveTurn,
        }
    );
    assert_eq!(
        codex.config_snapshot().await.approval_policy,
        AskForApproval::Never
    );
    assert_eq!(
        codex.environment_selections().await,
        steered_environments.environments
    );
    server.shutdown().await;
}

#[tokio::test]
async fn start_or_steer_turn_requires_matching_active_output_schema() {
    let (release_response, response_gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![
            StreamingSseChunk {
                gate: None,
                body: responses::sse(vec![ev_response_created("resp-1")]),
            },
            StreamingSseChunk {
                gate: Some(response_gate),
                body: responses::sse(vec![ev_completed("resp-1")]),
            },
        ],
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        }],
    ])
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.permissions.approval_policy = Constrained::allow_any(AskForApproval::OnRequest);
        })
        .build_with_streaming_server(&server)
        .await
        .expect("build turn-input submission session");
    let codex = &test.codex;
    let active_schema: Value = serde_json::from_str(
        r#"{"type":"object","properties":{"answer":{"type":"string"},"count":{"type":"number"}},"required":["answer","count"]}"#,
    )
    .expect("parse active schema");
    let matching_schema_with_different_object_order: Value = serde_json::from_str(
        r#"{"required":["answer","count"],"properties":{"count":{"type":"number"},"answer":{"type":"string"}},"type":"object"}"#,
    )
    .expect("parse matching schema");
    let different_schema: Value = serde_json::from_str(
        r#"{"type":"object","properties":{"answer":{"type":"number"}},"required":["answer"]}"#,
    )
    .expect("parse different schema");

    let started = codex
        .start_or_steer_turn(
            user_message_request("start turn").on_start(TurnStartOptions {
                final_output_json_schema: Some(active_schema),
                ..Default::default()
            }),
        )
        .await
        .expect("first message should start a turn");
    let TurnInputSubmission::Started { turn_id } = started else {
        panic!("first message should start a turn");
    };
    timeout(
        Duration::from_secs(5),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await
    .expect("started turn should reach its first model request");

    let rejected = codex
        .start_or_steer_turn(
            user_message_request("rejected steer")
                .with_thread_settings(ThreadSettingsOverrides {
                    approval_policy: Some(AskForApproval::Never),
                    ..Default::default()
                })
                .on_start(TurnStartOptions {
                    final_output_json_schema: Some(different_schema),
                    ..Default::default()
                }),
        )
        .await
        .expect("schema mismatch should return a typed rejection");
    assert_eq!(
        rejected,
        TurnInputSubmission::NotSubmitted {
            reason: NotSubmittedReason::ActiveTurnOutputSchemaMismatch,
        }
    );
    assert_eq!(
        codex.config_snapshot().await.approval_policy,
        AskForApproval::OnRequest
    );

    let steered = codex
        .start_or_steer_turn(
            user_message_request("accepted steer").on_start(TurnStartOptions {
                final_output_json_schema: Some(matching_schema_with_different_object_order),
                ..Default::default()
            }),
        )
        .await
        .expect("matching schema should steer");
    assert_eq!(steered, TurnInputSubmission::Steered { turn_id });

    release_response
        .send(())
        .expect("response gate should remain open");
    wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    let second_request = String::from_utf8_lossy(&requests[1]);
    assert!(second_request.contains("accepted steer"));
    assert!(!second_request.contains("rejected steer"));
    server.shutdown().await;
}
