use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::Mutex;

use codex_history::CodexHarnessMetadata;
use codex_history::InputAssociation;
use codex_history::InputIdentity;
use codex_history::InputSource;
use codex_history::InputStreamIncarnation;
use codex_history::ResponseItemEnvelope;
use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use tachyon_model::ModelContent;
use tachyon_model::ModelInputItem;
use tachyon_model::ModelMediaSource;
use tachyon_model::ModelMessage;
use tachyon_model::ModelMessageRole;
use tachyon_model::ModelReasoning;
use tachyon_model::ModelRequest;
use tachyon_model::ModelToolAvailability;
use tachyon_model::ModelToolCall;
use tachyon_model::ModelToolCallId;
use tachyon_model::ModelToolInput;
use tachyon_model::ModelToolPurpose;
use tachyon_model::ModelToolResult;
use tachyon_model::ModelToolResultContent;
use tachyon_model::ModelToolSpec;
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

use super::InitialContextInjection;
use super::prepare_backend_compaction;
use super::run_compact_task;
use super::run_inline_auto_compact_task;
use crate::model_runtime::ModelRuntime;

fn response_message(role: &str, text: &str, id: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::from_server(id.to_string())),
        role: role.to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(call_id: &str) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: "tool".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        encrypted_function_args: None,
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_output(call_id: &str) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id.to_string()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_text("done".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn custom_tool_call(call_id: &str) -> ResponseItem {
    ResponseItem::CustomToolCall {
        id: None,
        status: None,
        call_id: call_id.to_string(),
        name: "custom".to_string(),
        namespace: None,
        input: "input".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn custom_tool_output(call_id: &str) -> ResponseItem {
    ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: call_id.to_string(),
        name: None,
        output: FunctionCallOutputPayload::from_text("done".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn new_function_call(call_id: &str) -> ModelCompactionItem {
    ModelCompactionItem::NewItem(ModelInputItem::ToolCall(ModelToolCall {
        call_id: ModelToolCallId(call_id.to_string()),
        namespace: None,
        name: "tool".to_string(),
        input: ModelToolInput::Json(serde_json::Value::Null),
    }))
}

fn new_function_output(call_id: &str) -> ModelCompactionItem {
    ModelCompactionItem::NewItem(ModelInputItem::ToolResult(ModelToolResult {
        call_id: ModelToolCallId(call_id.to_string()),
        content: vec![ModelToolResultContent::Text("done".to_string())],
        is_error: None,
    }))
}

fn new_custom_call(call_id: &str) -> ModelCompactionItem {
    ModelCompactionItem::NewItem(ModelInputItem::ToolCall(ModelToolCall {
        call_id: ModelToolCallId(call_id.to_string()),
        namespace: None,
        name: "apply_patch".to_string(),
        input: ModelToolInput::Text("*** Begin Patch".to_string()),
    }))
}

fn new_tool_result(call_id: &str, content: Vec<ModelToolResultContent>) -> ModelCompactionItem {
    ModelCompactionItem::NewItem(ModelInputItem::ToolResult(ModelToolResult {
        call_id: ModelToolCallId(call_id.to_string()),
        content,
        is_error: Some(false),
    }))
}

fn envelope(item: ResponseItem) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(item)
}

fn metadata(reinjectable_context: bool) -> CodexHarnessMetadata {
    CodexHarnessMetadata {
        reinjectable_context,
        ..CodexHarnessMetadata::default()
    }
}

fn retained(index: usize) -> ModelCompactionItem {
    ModelCompactionItem::RetainedInput { index }
}

fn new_text(role: ModelMessageRole, text: &str) -> ModelCompactionItem {
    ModelCompactionItem::NewItem(ModelInputItem::Message(ModelMessage {
        role,
        phase: None,
        content: vec![ModelContent::Text(text.to_string())],
    }))
}

fn result(history: Vec<ModelCompactionItem>) -> ModelCompactionResult {
    ModelCompactionResult {
        history,
        usage: None,
    }
}

fn request(input_len: usize) -> ModelRequest {
    ModelRequest {
        input: (0..input_len)
            .map(|index| {
                ModelInputItem::Message(ModelMessage {
                    role: ModelMessageRole::User,
                    phase: None,
                    content: vec![ModelContent::Text(format!("slot {index}"))],
                })
            })
            .collect(),
        ..ModelRequest::default()
    }
}

#[test]
fn retained_equal_text_slots_keep_their_distinct_ids_and_metadata() {
    let first = ResponseItemEnvelope::with_metadata(
        response_message("user", "same text", "first"),
        CodexHarnessMetadata {
            fallback_token_limit_override: Some(11),
            ..metadata(/*reinjectable_context*/ false)
        },
    );
    let second = ResponseItemEnvelope::with_metadata(
        response_message("user", "same text", "second"),
        CodexHarnessMetadata {
            fallback_token_limit_override: Some(29),
            ..metadata(/*reinjectable_context*/ false)
        },
    );
    let snapshot = vec![first, second];
    let prepared = prepare_backend_compaction(
        &snapshot,
        &request(snapshot.len()),
        result(vec![retained(/*index*/ 0), retained(/*index*/ 1)]),
        "compact-turn",
    )
    .expect("both retained slots are valid");

    assert_eq!(prepared.history, snapshot);
    assert_eq!(
        prepared.history[0]
            .metadata
            .as_ref()
            .unwrap()
            .fallback_token_limit_override,
        Some(11)
    );
    assert_eq!(
        prepared.history[1]
            .metadata
            .as_ref()
            .unwrap()
            .fallback_token_limit_override,
        Some(29)
    );
    assert_eq!(prepared.history[0].item.id(), snapshot[0].item.id());
    assert_eq!(prepared.history[1].item.id(), snapshot[1].item.id());
}

#[test]
fn attached_notice_pairs_are_retained_together_and_in_order() {
    let notice = concat!(
        "<image_resize_notice>\n",
        "Image 1 of 1 in the preceding user message was resized from 2048x2048 to 1600x1600 pixels.\n",
        "</image_resize_notice>"
    );
    let snapshot = vec![
        ResponseItemEnvelope::with_metadata(
            response_message("user", "image source", "source"),
            metadata(/*reinjectable_context*/ true),
        ),
        ResponseItemEnvelope::with_metadata(
            response_message("developer", notice, "notice"),
            metadata(/*reinjectable_context*/ true),
        ),
    ];
    let pair = prepare_backend_compaction(
        &snapshot,
        &request(/*input_len*/ 2),
        result(vec![retained(/*index*/ 0), retained(/*index*/ 1)]),
        "compact-turn",
    )
    .expect("complete adjacent source/notice pair is valid");
    assert_eq!(pair.history, snapshot);

    let none = prepare_backend_compaction(
        &snapshot,
        &request(/*input_len*/ 2),
        result(Vec::new()),
        "compact-turn",
    )
    .expect("dropping the entire pair is valid");
    assert!(none.history.is_empty());

    let interleaved = vec![
        retained(/*index*/ 0),
        new_text(ModelMessageRole::Assistant, "summary"),
        retained(/*index*/ 1),
    ];
    for invalid in [
        vec![retained(/*index*/ 0)],
        vec![retained(/*index*/ 1), retained(/*index*/ 0)],
        interleaved,
    ] {
        assert!(
            prepare_backend_compaction(
                &snapshot,
                &request(/*input_len*/ 2),
                result(invalid),
                "compact-turn",
            )
            .is_err()
        );
    }
}

#[test]
fn prepared_history_requires_correlated_tool_pairs() {
    let empty_snapshot = Vec::new();
    let fresh_function_pair = prepare_backend_compaction(
        &empty_snapshot,
        &request(/*input_len*/ 0),
        result(vec![
            new_function_call("fresh-function"),
            new_function_output("fresh-function"),
        ]),
        "compact-turn",
    )
    .expect("complete fresh function call/output pair is valid");
    assert_eq!(fresh_function_pair.history.len(), 2);

    assert!(
        prepare_backend_compaction(
            &empty_snapshot,
            &request(/*input_len*/ 0),
            result(vec![new_function_call("fresh-call-only")]),
            "compact-turn",
        )
        .is_err()
    );

    assert!(
        prepare_backend_compaction(
            &empty_snapshot,
            &request(/*input_len*/ 0),
            result(vec![new_function_output("fresh-output-only")]),
            "compact-turn",
        )
        .is_err()
    );

    let function_pair = vec![
        envelope(function_call("function-1")),
        envelope(function_output("function-1")),
    ];
    let prepared = prepare_backend_compaction(
        &function_pair,
        &request(/*input_len*/ 2),
        result(vec![retained(/*index*/ 0), retained(/*index*/ 1)]),
        "compact-turn",
    )
    .expect("complete function call/output pair is valid");
    assert_eq!(prepared.history, function_pair);

    let mismatched_classes = vec![
        envelope(function_call("same-id")),
        envelope(custom_tool_output("same-id")),
    ];
    assert!(
        prepare_backend_compaction(
            &mismatched_classes,
            &request(/*input_len*/ 2),
            result(vec![retained(/*index*/ 0), retained(/*index*/ 1)]),
            "compact-turn",
        )
        .is_err()
    );

    let custom_pair = vec![
        envelope(custom_tool_call("custom-1")),
        envelope(custom_tool_output("custom-1")),
    ];
    let prepared = prepare_backend_compaction(
        &custom_pair,
        &request(/*input_len*/ 2),
        result(vec![retained(/*index*/ 0), retained(/*index*/ 1)]),
        "compact-turn",
    )
    .expect("complete retained custom-tool pair is valid");
    assert_eq!(prepared.history, custom_pair);
}

#[test]
fn prepared_history_projects_custom_and_discovery_results_from_matching_calls() {
    let custom = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![
            new_custom_call("fresh-custom"),
            new_tool_result(
                "fresh-custom",
                vec![ModelToolResultContent::Text("applied".to_string())],
            ),
        ]),
        "compact-turn",
    )
    .expect("fresh custom call/result pair is valid");
    assert!(matches!(
        custom.history.as_slice(),
        [
            ResponseItemEnvelope {
                item: ResponseItem::CustomToolCall { call_id, .. },
                ..
            },
            ResponseItemEnvelope {
                item: ResponseItem::CustomToolCallOutput { call_id: output_id, .. },
                ..
            }
        ] if call_id == "fresh-custom" && output_id == "fresh-custom"
    ));

    let retained_call = custom_tool_call("retained-custom");
    let retained_input = ModelInputItem::ToolCall(ModelToolCall {
        call_id: ModelToolCallId("retained-custom".to_string()),
        namespace: None,
        name: "custom".to_string(),
        input: ModelToolInput::Text("input".to_string()),
    });
    let retained_snapshot = vec![envelope(retained_call.clone())];
    let retained_result = prepare_backend_compaction(
        &retained_snapshot,
        &ModelRequest {
            input: vec![retained_input],
            ..ModelRequest::default()
        },
        result(vec![
            retained(/*index*/ 0),
            new_tool_result(
                "retained-custom",
                vec![ModelToolResultContent::Text("retained output".to_string())],
            ),
        ]),
        "compact-turn",
    )
    .expect("fresh output pairs with the exact retained custom call");
    assert_eq!(retained_result.history[0], retained_snapshot[0]);
    assert!(matches!(
        &retained_result.history[1].item,
        ResponseItem::CustomToolCallOutput { call_id, .. } if call_id == "retained-custom"
    ));

    let discovery_mismatch = result(vec![
        new_function_call("not-discovery"),
        new_tool_result(
            "not-discovery",
            vec![ModelToolResultContent::DiscoveredTools(Vec::new())],
        ),
    ]);
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            discovery_mismatch,
            "compact-turn"
        )
        .is_err()
    );

    let discovery_request = ModelRequest {
        tools: vec![ModelToolSpec::Function {
            namespace: None,
            name: "tool_search".to_string(),
            description: "Find additional tools".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            strict: false,
            availability: ModelToolAvailability::Immediate,
            purpose: ModelToolPurpose::Discovery,
        }],
        ..ModelRequest::default()
    };
    let discovery = prepare_backend_compaction(
        &[],
        &discovery_request,
        result(vec![
            ModelCompactionItem::NewItem(ModelInputItem::ToolCall(ModelToolCall {
                call_id: ModelToolCallId("fresh-discovery".to_string()),
                namespace: None,
                name: "tool_search".to_string(),
                input: ModelToolInput::Json(serde_json::json!({"query": "tools"})),
            })),
            new_tool_result(
                "fresh-discovery",
                vec![ModelToolResultContent::DiscoveredTools(Vec::new())],
            ),
        ]),
        "compact-turn",
    )
    .expect("declared client discovery call/result pair is valid");
    assert!(matches!(
        discovery.history.as_slice(),
        [
            ResponseItemEnvelope {
                item: ResponseItem::ToolSearchCall { call_id: Some(call_id), execution, .. },
                ..
            },
            ResponseItemEnvelope {
                item: ResponseItem::ToolSearchOutput { call_id: Some(output_id), execution: output_execution, .. },
                ..
            }
        ] if call_id == "fresh-discovery"
            && output_id == "fresh-discovery"
            && execution == "client"
            && output_execution == "client"
    ));
}

#[test]
fn fresh_tool_results_reject_missing_or_ambiguous_projected_calls() {
    let duplicate_call_id = result(vec![
        new_function_call("duplicate"),
        new_custom_call("duplicate"),
        new_function_output("duplicate"),
    ]);
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            duplicate_call_id,
            "compact-turn"
        )
        .is_err()
    );
    let mismatched_call_id = result(vec![
        new_function_call("call-id-one"),
        new_function_output("call-id-two"),
    ]);
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            mismatched_call_id,
            "compact-turn"
        )
        .is_err()
    );
}

#[test]
fn provider_owned_tool_search_output_does_not_require_a_local_call() {
    let snapshot = vec![envelope(ResponseItem::ToolSearchOutput {
        id: None,
        call_id: Some("server-search".to_string()),
        status: "completed".to_string(),
        execution: "server".to_string(),
        tools: Vec::new(),
        internal_chat_message_metadata_passthrough: None,
    })];
    let prepared = prepare_backend_compaction(
        &snapshot,
        &request(/*input_len*/ 1),
        result(vec![retained(/*index*/ 0)]),
        "compact-turn",
    )
    .expect("server-owned output is explicitly allowed without a local call");
    assert_eq!(prepared.history, snapshot);
}

#[test]
fn only_unassociated_nonclient_singleton_context_is_dropped() {
    let associated = InputAssociation {
        identity: InputIdentity {
            thread_id: ThreadId::default(),
            incarnation: InputStreamIncarnation::new(),
            sequence: NonZeroU64::new(1).expect("one is nonzero"),
        },
        source: InputSource::Synthetic,
    };
    let snapshot = vec![
        ResponseItemEnvelope::with_metadata(
            response_message("developer", "rebuildable", "reinject"),
            metadata(/*reinjectable_context*/ true),
        ),
        ResponseItemEnvelope::with_metadata(
            response_message("developer", "client-authored", "client"),
            CodexHarnessMetadata {
                client_authored: true,
                reinjectable_context: true,
                ..CodexHarnessMetadata::default()
            },
        ),
        ResponseItemEnvelope::with_metadata(
            response_message("user", "input-associated", "associated"),
            CodexHarnessMetadata {
                reinjectable_context: true,
                input_association: Some(associated),
                ..CodexHarnessMetadata::default()
            },
        ),
        envelope(response_message(
            "assistant",
            "unknown provenance",
            "unknown",
        )),
    ];
    let prepared = prepare_backend_compaction(
        &snapshot,
        &request(snapshot.len()),
        result((0..snapshot.len()).map(retained).collect()),
        "compact-turn",
    )
    .expect("all four singleton references are valid");

    assert_eq!(prepared.history, snapshot[1..]);
}

#[test]
fn backend_order_is_preserved_for_independent_retained_items() {
    let snapshot = vec![
        ResponseItemEnvelope::with_metadata(
            response_message("user", "first", "first"),
            CodexHarnessMetadata {
                fallback_token_limit_override: Some(3),
                ..metadata(/*reinjectable_context*/ false)
            },
        ),
        ResponseItemEnvelope::with_metadata(
            response_message("user", "second", "second"),
            CodexHarnessMetadata {
                fallback_token_limit_override: Some(8),
                ..metadata(/*reinjectable_context*/ false)
            },
        ),
    ];
    let prepared = prepare_backend_compaction(
        &snapshot,
        &request(/*input_len*/ 2),
        result(vec![retained(/*index*/ 1), retained(/*index*/ 0)]),
        "compact-turn",
    )
    .expect("backend may order independent retained items");
    assert_eq!(
        prepared.history,
        vec![snapshot[1].clone(), snapshot[0].clone()]
    );
}

#[test]
fn fresh_items_are_stamped_without_history_metadata_and_invalid_results_fail_whole() {
    let empty_snapshot = Vec::new();
    let fresh = prepare_backend_compaction(
        &empty_snapshot,
        &request(/*input_len*/ 0),
        result(vec![new_text(ModelMessageRole::Developer, "fresh context")]),
        "compact-turn",
    )
    .expect("representable fresh message converts");
    assert_eq!(fresh.history.len(), 1);
    assert!(fresh.history[0].metadata.is_none());
    assert_eq!(fresh.history[0].item.turn_id(), Some("compact-turn"));

    let original = result(vec![retained(/*index*/ 0)]);
    let unchanged = original.clone();
    assert!(
        prepare_backend_compaction(&[], &request(/*input_len*/ 0), original, "compact-turn")
            .is_err()
    );
    assert_eq!(unchanged, result(vec![retained(/*index*/ 0)]));
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            result(vec![retained(usize::MAX)]),
            "compact-turn",
        )
        .is_err()
    );

    let duplicate = result(vec![retained(/*index*/ 0), retained(/*index*/ 0)]);
    assert!(
        prepare_backend_compaction(
            &[envelope(response_message("user", "one", "one"))],
            &request(/*input_len*/ 1),
            duplicate,
            "compact-turn",
        )
        .is_err()
    );

    let usage_overflow = ModelCompactionResult {
        history: Vec::new(),
        usage: Some(ModelUsage {
            input_tokens: u64::MAX,
            output_tokens: 0,
            ..ModelUsage::default()
        }),
    };
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            usage_overflow,
            "compact-turn"
        )
        .is_err()
    );

    let unrepresentable_bytes = result(vec![ModelCompactionItem::NewItem(
        ModelInputItem::Message(ModelMessage {
            role: ModelMessageRole::User,
            phase: None,
            content: vec![ModelContent::Image {
                source: ModelMediaSource::Bytes {
                    media_type: "image/png".to_string(),
                    data: Arc::<[u8]>::from(vec![1u8, 2u8]),
                },
                detail: None,
            }],
        }),
    )]);
    assert!(
        prepare_backend_compaction(
            &[],
            &request(/*input_len*/ 0),
            unrepresentable_bytes,
            "compact-turn"
        )
        .is_err()
    );
}

fn assert_estimated_item_limit_error(error: &codex_protocol::error::CodexErr) {
    assert!(matches!(
        error.details(),
        codex_protocol::error::CodexErrorDetails::InvalidRequest(message)
            if message.contains("exceeds the 10000-token estimated item limit")
                && message.contains("estimate is approximate")
    ));
}

#[test]
fn fresh_backend_items_obey_estimated_item_limit_but_retained_items_do_not() {
    let small = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![new_text(ModelMessageRole::Assistant, "small summary")]),
        "compact-turn",
    )
    .expect("a below-limit fresh item is accepted");
    assert_eq!(small.history.len(), 1);

    let oversized = "x".repeat(50_000);
    let oversized_message = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![new_text(ModelMessageRole::Assistant, &oversized)]),
        "compact-turn",
    )
    .err()
    .expect("oversized fresh message is rejected");
    assert_estimated_item_limit_error(&oversized_message);

    let oversized_reasoning = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![ModelCompactionItem::NewItem(
            ModelInputItem::Reasoning(ModelReasoning {
                summary: vec![oversized.clone()],
                content: Vec::new(),
            }),
        )]),
        "compact-turn",
    )
    .err()
    .expect("oversized fresh reasoning is rejected");
    assert_estimated_item_limit_error(&oversized_reasoning);

    let oversized_call = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![
            ModelCompactionItem::NewItem(ModelInputItem::ToolCall(ModelToolCall {
                call_id: ModelToolCallId("large-call".to_string()),
                namespace: None,
                name: "tool".to_string(),
                input: ModelToolInput::Json(serde_json::json!({"payload": oversized})),
            })),
            new_function_output("large-call"),
        ]),
        "compact-turn",
    )
    .err()
    .expect("oversized fresh tool call is rejected");
    assert_estimated_item_limit_error(&oversized_call);

    let oversized_result = prepare_backend_compaction(
        &[],
        &request(/*input_len*/ 0),
        result(vec![
            new_function_call("large-result"),
            new_tool_result(
                "large-result",
                vec![ModelToolResultContent::Text(oversized.clone())],
            ),
        ]),
        "compact-turn",
    )
    .err()
    .expect("oversized fresh tool result is rejected");
    assert_estimated_item_limit_error(&oversized_result);

    let retained_snapshot = vec![envelope(response_message("user", &oversized, "large"))];
    let retained_large = prepare_backend_compaction(
        &retained_snapshot,
        &request(/*input_len*/ 1),
        result(vec![retained(/*index*/ 0)]),
        "compact-turn",
    )
    .expect("existing oversized retained history remains unchanged");
    assert_eq!(retained_large.history, retained_snapshot);
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedCall {
    turn_id: usize,
    route: ModelRoute,
    model_id: String,
    request: ModelRequest,
}

#[derive(Debug, Default)]
struct FakeState {
    turns_created: usize,
    calls: Vec<RecordedCall>,
    stream_calls: usize,
}

#[derive(Debug, Clone)]
struct FakeBackend {
    state: Arc<Mutex<FakeState>>,
    route_provider_override: Option<ModelProviderId>,
    outcome: Result<ModelCompactionResult, ModelBackendError>,
}

impl ModelBackend for FakeBackend {
    fn begin_turn(&self, provider_id: ModelProviderId) -> Box<dyn ModelTurnBackend> {
        let turn_id = {
            let mut state = self.state.lock().expect("fake backend state lock poisoned");
            let turn_id = state.turns_created;
            state.turns_created += 1;
            turn_id
        };
        Box::new(FakeTurnBackend {
            turn_id,
            route: ModelRoute::new(
                self.route_provider_override.clone().unwrap_or(provider_id),
                ModelProtocol::new("test.protocol"),
                ModelTransport::Http,
            ),
            state: Arc::clone(&self.state),
            outcome: self.outcome.clone(),
        })
    }

    fn supports_compaction(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct FakeTurnBackend {
    turn_id: usize,
    route: ModelRoute,
    state: Arc<Mutex<FakeState>>,
    outcome: Result<ModelCompactionResult, ModelBackendError>,
}

impl ModelTurnBackend for FakeTurnBackend {
    fn route(&self) -> &ModelRoute {
        &self.route
    }

    fn stream<'a>(
        &'a mut self,
        _model_id: &'a str,
        _request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelEventStream, ModelBackendError>> {
        self.state
            .lock()
            .expect("fake backend state lock poisoned")
            .stream_calls += 1;
        Box::pin(std::future::ready(Err(
            ModelBackendError::UnsupportedRequest("unexpected sampling fallback".to_string()),
        )))
    }

    fn compact<'a>(
        &'a mut self,
        model_id: &'a str,
        request: &'a ModelRequest,
    ) -> ModelBackendFuture<'a, Result<ModelCompactionResult, ModelBackendError>> {
        self.state
            .lock()
            .expect("fake backend state lock poisoned")
            .calls
            .push(RecordedCall {
                turn_id: self.turn_id,
                route: self.route.clone(),
                model_id: model_id.to_string(),
                request: request.clone(),
            });
        Box::pin(std::future::ready(self.outcome.clone()))
    }
}

fn fake_runtime(
    route_provider_override: Option<ModelProviderId>,
    outcome: Result<ModelCompactionResult, ModelBackendError>,
) -> (ModelRuntime, Arc<Mutex<FakeState>>) {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let backend = FakeBackend {
        state: Arc::clone(&state),
        route_provider_override,
        outcome,
    };
    (ModelRuntime::from_backend(Arc::new(backend)), state)
}

async fn canonical_session(
    runtime: &ModelRuntime,
) -> (
    Arc<crate::session::session::Session>,
    Arc<crate::session::turn_context::TurnContext>,
    Arc<crate::session::step_context::StepContext>,
) {
    let (mut session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    session.services.model_runtime = runtime.clone();
    let mut config = turn_context.config.as_ref().clone();
    config
        .web_search_mode
        .set(WebSearchMode::Disabled)
        .expect("disable hosted web search for canonical compaction fixture");
    turn_context.config = Arc::new(config);
    let session = Arc::new(session);
    let turn_context = Arc::new(turn_context);
    let step_context =
        crate::session::step_context::StepContext::for_test(Arc::clone(&turn_context));
    session
        .replace_history(
            vec![response_message("user", "existing history", "existing")],
            /*reference_context_item*/ None,
        )
        .await;
    (session, turn_context, step_context)
}

fn backend_summary() -> ModelCompactionResult {
    ModelCompactionResult {
        history: vec![new_text(ModelMessageRole::Assistant, "backend summary")],
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

#[tokio::test]
async fn inline_reuses_its_turn_handle_and_manual_compaction_uses_a_fresh_one() {
    let (runtime, state) =
        fake_runtime(/*route_provider_override*/ None, Ok(backend_summary()));
    let (session, turn_context, step_context) = canonical_session(&runtime).await;
    let before_window = session.current_window().await;
    let mut inline_runtime =
        runtime.begin_turn_for_provider(turn_context.config.model_provider_id.clone());

    for _ in 0..2 {
        run_inline_auto_compact_task(
            Arc::clone(&session),
            Arc::clone(&step_context),
            &mut inline_runtime,
            InitialContextInjection::DoNotInject,
            codex_analytics::CompactionReason::ContextLimit,
            codex_analytics::CompactionPhase::MidTurn,
        )
        .await
        .expect("inline backend compaction succeeds");
    }
    run_compact_task(Arc::clone(&session), Arc::clone(&turn_context))
        .await
        .expect("manual backend compaction succeeds on a new handle");

    {
        let state = state.lock().expect("fake backend state lock poisoned");
        assert_eq!(state.turns_created, 2);
        assert_eq!(state.calls.len(), 3);
        assert_eq!(state.calls[0].turn_id, 0);
        assert_eq!(state.calls[1].turn_id, 0);
        assert_eq!(state.calls[2].turn_id, 1);
        assert!(state.calls.iter().all(|call| {
            call.model_id.as_str() == turn_context.model_info().slug.as_str()
                && call.route.provider_id().id() == turn_context.config.model_provider_id.as_str()
        }));
        assert_eq!(
            state.calls[0].request.input,
            vec![ModelInputItem::Message(ModelMessage {
                role: ModelMessageRole::User,
                phase: None,
                content: vec![ModelContent::Text("existing history".to_string())],
            })]
        );
        assert_eq!(
            state.calls[1].request.input,
            vec![ModelInputItem::Message(ModelMessage {
                role: ModelMessageRole::Assistant,
                phase: None,
                content: vec![ModelContent::Text("backend summary".to_string())],
            })]
        );
        assert_eq!(state.stream_calls, 0);
    }
    let after_window = session.current_window().await;
    assert_ne!(after_window, before_window);
    let installed_history = session.clone_history().await.into_annotated_items();
    assert!(matches!(
        installed_history.as_slice(),
        [envelope]
            if matches!(
                &envelope.item,
                ResponseItem::Message { role, content, .. }
                    if role == "assistant"
                        && matches!(content.as_slice(), [ContentItem::OutputText { text }] if text == "backend summary")
            )
    ));
}

#[tokio::test]
async fn inline_compaction_reinjects_context_for_the_new_window_and_reference() {
    let (runtime, _) = fake_runtime(/*route_provider_override*/ None, Ok(backend_summary()));
    let (session, turn_context, step_context) = canonical_session(&runtime).await;
    let before_window = session.current_window().await;
    let world_state = Arc::new(
        session
            .build_world_state_for_step(&step_context)
            .await
            .expect("step world state builds"),
    );
    let mut inline_runtime =
        runtime.begin_turn_for_provider(turn_context.config.model_provider_id.clone());

    run_inline_auto_compact_task(
        Arc::clone(&session),
        Arc::clone(&step_context),
        &mut inline_runtime,
        InitialContextInjection::BeforeLastUserMessage {
            world_state: Arc::clone(&world_state),
            step_context: Arc::clone(&step_context),
        },
        codex_analytics::CompactionReason::ContextLimit,
        codex_analytics::CompactionPhase::MidTurn,
    )
    .await
    .expect("inline compaction with full context injection succeeds");

    let after_window = session.current_window().await;
    assert_ne!(after_window.0, before_window.0);
    assert_ne!(after_window.1, before_window.1);
    assert_eq!(
        session.reference_context_item().await,
        Some(turn_context.to_turn_context_item())
    );
    let expected_context = session
        .build_initial_context_with_world_state(&turn_context, world_state.as_ref())
        .await;
    assert!(!expected_context.is_empty());
    let installed_history = session.clone_history().await.into_annotated_items();
    assert_eq!(installed_history.len(), expected_context.len() + 1);
    for (expected, installed) in expected_context
        .iter()
        .zip(installed_history.iter().take(expected_context.len()))
    {
        let ResponseItem::Message { id: None, .. } = expected else {
            panic!("full context builder should produce fresh message items without IDs");
        };
        let ResponseItem::Message { id: Some(_), .. } = &installed.item else {
            panic!("installed full context receives a history item ID");
        };
        let mut installed_without_id = installed.item.clone();
        if let ResponseItem::Message { id, .. } = &mut installed_without_id {
            *id = None;
        }
        assert_eq!(&installed_without_id, expected);
        assert!(
            installed
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.reinjectable_context)
        );
    }
    assert!(matches!(
        &installed_history[expected_context.len()].item,
        ResponseItem::Message { role, content, .. }
            if role == "assistant"
                && matches!(content.as_slice(), [ContentItem::OutputText { text }] if text == "backend summary")
    ));
}

#[tokio::test]
async fn backend_errors_do_not_fall_back_or_mutate_history_window() {
    let (runtime, state) = fake_runtime(
        /*route_provider_override*/ None,
        Err(ModelBackendError::Failed {
            message: "backend failure".to_string(),
            retryable: false,
        }),
    );
    let (session, turn_context, step_context) = canonical_session(&runtime).await;
    let mut turn_runtime =
        runtime.begin_turn_for_provider(turn_context.config.model_provider_id.clone());
    let before_history = session.clone_history().await.into_annotated_items();
    let before_window = session.current_window_id().await;

    assert!(
        run_inline_auto_compact_task(
            Arc::clone(&session),
            step_context,
            &mut turn_runtime,
            InitialContextInjection::DoNotInject,
            codex_analytics::CompactionReason::ContextLimit,
            codex_analytics::CompactionPhase::MidTurn,
        )
        .await
        .is_err()
    );
    assert_eq!(
        session.clone_history().await.into_annotated_items(),
        before_history
    );
    assert_eq!(session.current_window_id().await, before_window);
    let state = state.lock().expect("fake backend state lock poisoned");
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.stream_calls, 0);
}

#[tokio::test]
async fn oversized_backend_compaction_does_not_install_history_usage_or_window() {
    let oversized = "x".repeat(50_000);
    let (runtime, state) = fake_runtime(
        /*route_provider_override*/ None,
        Ok(ModelCompactionResult {
            history: vec![new_text(ModelMessageRole::Assistant, &oversized)],
            usage: Some(ModelUsage {
                input_tokens: 23,
                output_tokens: 7,
                cached_input_tokens: Some(3),
                ..ModelUsage::default()
            }),
        }),
    );
    let (session, turn_context, step_context) = canonical_session(&runtime).await;
    let mut turn_runtime =
        runtime.begin_turn_for_provider(turn_context.config.model_provider_id.clone());
    let before_history = session.clone_history().await.into_annotated_items();
    let before_window = session.current_window_id().await;
    let before_total_usage = session.get_total_token_usage().await;

    let error = run_inline_auto_compact_task(
        Arc::clone(&session),
        step_context,
        &mut turn_runtime,
        InitialContextInjection::DoNotInject,
        codex_analytics::CompactionReason::ContextLimit,
        codex_analytics::CompactionPhase::MidTurn,
    )
    .await
    .expect_err("oversized backend history must fail before installation");
    assert_estimated_item_limit_error(&error);
    assert_eq!(
        session.clone_history().await.into_annotated_items(),
        before_history
    );
    assert_eq!(session.current_window_id().await, before_window);
    assert_eq!(session.get_total_token_usage().await, before_total_usage);

    let state = state.lock().expect("fake backend state lock poisoned");
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.stream_calls, 0);
}
