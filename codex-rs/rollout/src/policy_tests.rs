use std::borrow::Cow;
use std::time::Duration;

use codex_protocol::ThreadId;
use codex_protocol::items::CommandExecutionItem;
use codex_protocol::items::CommandExecutionStatus;
use codex_protocol::items::McpToolCallError;
use codex_protocol::items::McpToolCallItem;
use codex_protocol::items::McpToolCallStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecCommandSource;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_utils_string::truncate_middle_with_marker;
use pretty_assertions::assert_eq;

use super::COMMAND_OUTPUT_TRUNCATION_MARKER;
use super::MCP_TOOL_RESULT_TRUNCATION_MARKER;
use super::PERSISTED_COMMAND_OUTPUT_MAX_BYTES;
use super::PERSISTED_MCP_TOOL_RESULT_MAX_BYTES;
use super::PERSISTED_MCP_TOOL_RESULT_PREVIEW_MAX_BYTES;
use super::into_persisted_rollout_items;
use super::is_persisted_rollout_item;
use super::persisted_rollout_item;
use super::should_persist_event_msg;
use crate::RolloutItem;
use crate::RolloutLine;

pub(crate) fn command_item(outputs: [Option<String>; 4]) -> RolloutItem {
    let [stdout, stderr, aggregated_output, formatted_output] = outputs;
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id: ThreadId::default(),
        turn_id: "turn".to_string(),
        item: TurnItem::CommandExecution(CommandExecutionItem {
            id: "command".to_string(),
            plugin_id: Some("plugin".to_string()),
            script_path: Some("scripts/run.sh".to_string()),
            process_id: Some("process".to_string()),
            command: vec!["echo".to_string(), "hello".to_string()],
            cwd: serde_json::from_value(serde_json::json!("file:///tmp"))
                .expect("valid command cwd"),
            parsed_cmd: vec![ParsedCommand::Unknown {
                cmd: "echo hello".to_string(),
            }],
            source: ExecCommandSource::Agent,
            interaction_input: Some("input remains intact".to_string()),
            status: CommandExecutionStatus::Completed,
            stdout,
            stderr,
            aggregated_output,
            exit_code: Some(0),
            duration: None,
            formatted_output,
        }),
        started_at_ms: Some(1),
        completed_at_ms: 2,
    }))
}

fn command(item: &RolloutItem) -> &CommandExecutionItem {
    let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = item else {
        panic!("expected item-completed event")
    };
    let TurnItem::CommandExecution(command) = &event.item else {
        panic!("expected command execution item")
    };
    command
}

fn command_mut(item: &mut RolloutItem) -> &mut CommandExecutionItem {
    let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = item else {
        panic!("expected item-completed event")
    };
    let TurnItem::CommandExecution(command) = &mut event.item else {
        panic!("expected command execution item")
    };
    command
}

fn outputs(command: &CommandExecutionItem) -> [Option<&str>; 4] {
    [
        command.stdout.as_deref(),
        command.stderr.as_deref(),
        command.aggregated_output.as_deref(),
        command.formatted_output.as_deref(),
    ]
}

fn mcp_tool_result_item(result: Option<CallToolResult>) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id: ThreadId::default(),
        turn_id: "mcp-turn".to_string(),
        item: TurnItem::McpToolCall(McpToolCallItem {
            id: "mcp-call-id".to_string(),
            server: "example-server".to_string(),
            tool: "example-tool".to_string(),
            arguments: serde_json::json!({"query": "arguments remain intact"}),
            connector_id: Some("connector".to_string()),
            mcp_app_resource_uri: Some("mcp://app/resource".to_string()),
            link_id: Some("link".to_string()),
            app_name: Some("app".to_string()),
            action_name: Some("action".to_string()),
            plugin_id: Some("plugin".to_string()),
            read_only_hint: Some(true),
            status: McpToolCallStatus::Completed,
            result,
            error: Some(McpToolCallError {
                message: "item error remains intact".to_string(),
            }),
            duration: Some(Duration::from_millis(42)),
        }),
        started_at_ms: Some(123),
        completed_at_ms: 456,
    }))
}

#[test]
fn oversized_command_output_fields_are_projected_independently_without_mutating_input() {
    let short = Some("short output".to_string());
    let original_outputs = [
        Some("original stdout".to_string()),
        short.clone(),
        None,
        None,
    ];
    let oversized = format!(
        "prefix:{}:suffix",
        "💥".repeat(PERSISTED_COMMAND_OUTPUT_MAX_BYTES)
    );

    for oversized_field in 0..4 {
        let mut fields = original_outputs.clone();
        fields[oversized_field] = Some(oversized.clone());
        let original = command_item(fields);
        let original_json = serde_json::to_value(&original).expect("serialize original item");

        let projected = persisted_rollout_item(&original, ThreadHistoryMode::Paginated)
            .expect("paginated command item is retained");
        let Cow::Owned(projected) = projected else {
            panic!("oversized command output should be owned")
        };

        assert_eq!(
            serde_json::to_value(&original).expect("serialize unchanged original"),
            original_json
        );
        let output = outputs(command(&projected))[oversized_field].expect("output remains present");
        assert!(output.len() <= PERSISTED_COMMAND_OUTPUT_MAX_BYTES);
        assert!(output.contains(COMMAND_OUTPUT_TRUNCATION_MARKER));
        assert!(output.starts_with("prefix:") && output.ends_with(":suffix"));
        let original_command = command(&original);
        let projected_command = command(&projected);
        let original_outputs = outputs(original_command);
        let projected_outputs = outputs(projected_command);
        for index in 0..4 {
            if index != oversized_field {
                assert_eq!(projected_outputs[index], original_outputs[index]);
            }
        }
        let mut expected = original.clone();
        let expected_command = command_mut(&mut expected);
        expected_command.stdout = projected_command.stdout.clone();
        expected_command.stderr = projected_command.stderr.clone();
        expected_command.aggregated_output = projected_command.aggregated_output.clone();
        expected_command.formatted_output = projected_command.formatted_output.clone();
        assert_eq!(
            serde_json::to_value(expected).expect("serialize expected projection"),
            serde_json::to_value(&projected).expect("serialize actual projection")
        );
    }

    let item = command_item([
        Some(oversized.clone()),
        Some(oversized.clone()),
        None,
        Some(oversized),
    ]);
    let projected = persisted_rollout_item(&item, ThreadHistoryMode::Paginated)
        .expect("paginated command item is retained");
    let projected_outputs = outputs(command(projected.as_ref()));
    assert_eq!(projected_outputs[2], None);
    assert!([0, 1, 3].into_iter().all(|index| {
        projected_outputs[index]
            .is_some_and(|output| output.len() <= PERSISTED_COMMAND_OUTPUT_MAX_BYTES)
    }));

    let once = persisted_rollout_item(&item, ThreadHistoryMode::Paginated).unwrap();
    let twice = persisted_rollout_item(once.as_ref(), ThreadHistoryMode::Paginated).unwrap();
    assert!(matches!(&twice, Cow::Borrowed(_)));
    let once_json = serde_json::to_value(once.as_ref()).unwrap();
    assert_eq!(serde_json::to_value(twice.as_ref()).unwrap(), once_json);
    let moved = into_persisted_rollout_items(vec![item.clone()], ThreadHistoryMode::Paginated);
    assert_eq!(
        serde_json::to_value(moved).unwrap(),
        serde_json::json!([once_json])
    );
}

#[test]
fn exact_limit_short_and_none_outputs_remain_borrowed_and_legacy_eligibility_is_unchanged() {
    let item = command_item([
        Some("x".repeat(PERSISTED_COMMAND_OUTPUT_MAX_BYTES)),
        Some("short stderr".to_string()),
        None,
        None,
    ]);
    let before = serde_json::to_value(&item).expect("serialize item");

    let projected = persisted_rollout_item(&item, ThreadHistoryMode::Paginated)
        .expect("paginated command item is retained");
    assert!(matches!(&projected, Cow::Borrowed(_)));
    assert_eq!(
        serde_json::to_value(projected.as_ref()).expect("serialize unchanged projection"),
        before,
    );
    let event = match &item {
        RolloutItem::EventMsg(event) => event,
        _ => unreachable!(),
    };

    assert!(!is_persisted_rollout_item(&item, ThreadHistoryMode::Legacy));
    assert!(!should_persist_event_msg(event, ThreadHistoryMode::Legacy));
    assert!(persisted_rollout_item(&item, ThreadHistoryMode::Legacy).is_none());
}

#[test]
fn oversized_mcp_results_are_projected_with_a_serialized_result_cap_and_preserve_the_call() {
    let escaped_non_ascii = "雪💥\"\\\n".repeat(10_000);
    let small_content = vec![serde_json::json!({"type": "text", "text": "small content"})];
    let cases = [
        CallToolResult {
            content: vec![serde_json::json!({
                "type": "text",
                "text": escaped_non_ascii.clone(),
            })],
            structured_content: Some(serde_json::json!({"retained": "only before projection"})),
            is_error: Some(true),
            meta: Some(serde_json::json!({"retained": "only before projection"})),
        },
        CallToolResult {
            content: small_content.clone(),
            structured_content: Some(serde_json::json!({"payload": escaped_non_ascii.clone()})),
            is_error: Some(false),
            meta: Some(serde_json::json!({"retained": "only before projection"})),
        },
        CallToolResult {
            content: small_content,
            structured_content: Some(serde_json::json!({"small": true})),
            is_error: None,
            meta: Some(serde_json::json!({"payload": escaped_non_ascii})),
        },
    ];

    for original_result in cases {
        let serialized_result =
            serde_json::to_string(&original_result).expect("serialize original MCP result");
        assert!(serialized_result.len() > PERSISTED_MCP_TOOL_RESULT_MAX_BYTES);

        let original = mcp_tool_result_item(Some(original_result.clone()));
        let original_json = serde_json::to_value(&original).expect("serialize original item");
        let persisted = persisted_rollout_item(&original, ThreadHistoryMode::Paginated)
            .expect("paginated MCP call is retained");
        let Cow::Owned(projected) = persisted else {
            panic!("oversized MCP result should be owned")
        };
        assert_eq!(
            serde_json::to_value(&original).expect("serialize unchanged live item"),
            original_json
        );

        let expected_preview = truncate_middle_with_marker(
            &serialized_result,
            PERSISTED_MCP_TOOL_RESULT_PREVIEW_MAX_BYTES,
            MCP_TOOL_RESULT_TRUNCATION_MARKER,
        );
        assert!(expected_preview.len() <= PERSISTED_MCP_TOOL_RESULT_PREVIEW_MAX_BYTES);
        assert!(expected_preview.contains(MCP_TOOL_RESULT_TRUNCATION_MARKER));
        let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = &projected else {
            panic!("expected item-completed event")
        };
        let TurnItem::McpToolCall(tool_call) = &event.item else {
            panic!("expected MCP tool call item")
        };
        let projected_result = tool_call.result.as_ref().expect("MCP tool result");
        assert_eq!(projected_result.content.len(), 1);
        assert_eq!(
            projected_result.content[0],
            serde_json::json!({"type": "text", "text": expected_preview})
        );
        assert_eq!(projected_result.structured_content, None);
        assert_eq!(projected_result.meta, None);
        assert_eq!(projected_result.is_error, original_result.is_error);
        assert!(
            serde_json::to_string(projected_result)
                .expect("serialize projected MCP result")
                .len()
                <= PERSISTED_MCP_TOOL_RESULT_MAX_BYTES
        );

        let mut expected = original.clone();
        let RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) = &mut expected else {
            unreachable!()
        };
        let TurnItem::McpToolCall(tool_call) = &mut event.item else {
            unreachable!()
        };
        tool_call.result = Some((*projected_result).clone());
        assert_eq!(
            serde_json::to_value(expected).expect("serialize expected projection"),
            serde_json::to_value(&projected).expect("serialize actual projection")
        );

        let once = persisted_rollout_item(&projected, ThreadHistoryMode::Paginated)
            .expect("projected item remains retained");
        assert!(matches!(&once, Cow::Borrowed(_)));
        assert_eq!(
            serde_json::to_value(once.as_ref()).expect("serialize first projection"),
            serde_json::to_value(&projected).expect("serialize projected item")
        );

        let moved =
            into_persisted_rollout_items(vec![original.clone()], ThreadHistoryMode::Paginated);
        assert_eq!(
            serde_json::to_value(moved).expect("serialize owned persistence batch"),
            serde_json::json!([serde_json::to_value(&projected).unwrap()])
        );

        let line = RolloutLine {
            timestamp: "2026-10-15T00:00:00.000Z".to_string(),
            ordinal: Some(9),
            item: projected,
        };
        let encoded = serde_json::to_string(&line).expect("serialize projected rollout line");
        let decoded = crate::parse_rollout_line(&encoded).expect("parse projected rollout line");
        assert_eq!(
            serde_json::to_value(decoded).expect("serialize decoded rollout line"),
            serde_json::to_value(line).expect("serialize original rollout line")
        );
    }
}

#[test]
fn exact_limit_small_and_missing_mcp_results_remain_borrowed_and_legacy_is_unchanged() {
    let empty_result = CallToolResult {
        content: vec![serde_json::json!({"type": "text", "text": ""})],
        structured_content: None,
        is_error: None,
        meta: None,
    };
    let empty_result_json = serde_json::to_string(&empty_result).expect("serialize base result");
    let exact_result = CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": "x".repeat(PERSISTED_MCP_TOOL_RESULT_MAX_BYTES - empty_result_json.len()),
        })],
        ..empty_result
    };
    assert_eq!(
        serde_json::to_string(&exact_result)
            .expect("serialize exact-limit result")
            .len(),
        PERSISTED_MCP_TOOL_RESULT_MAX_BYTES
    );

    for result in [
        exact_result,
        CallToolResult {
            content: vec![serde_json::json!({"type": "text", "text": "small"})],
            structured_content: None,
            is_error: Some(false),
            meta: None,
        },
    ] {
        let item = mcp_tool_result_item(Some(result));
        let before = serde_json::to_value(&item).expect("serialize item");
        let persisted = persisted_rollout_item(&item, ThreadHistoryMode::Paginated)
            .expect("paginated MCP call is retained");
        assert!(matches!(&persisted, Cow::Borrowed(_)));
        assert_eq!(
            serde_json::to_value(persisted.as_ref()).expect("serialize unchanged projection"),
            before
        );
    }

    let missing_result = mcp_tool_result_item(/*result*/ None);
    let missing_before = serde_json::to_value(&missing_result).expect("serialize missing result");
    let missing_projection = persisted_rollout_item(&missing_result, ThreadHistoryMode::Paginated)
        .expect("paginated MCP call with no result is retained");
    assert!(matches!(&missing_projection, Cow::Borrowed(_)));
    assert_eq!(
        serde_json::to_value(missing_projection.as_ref()).expect("serialize missing projection"),
        missing_before
    );

    let oversized = mcp_tool_result_item(Some(CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": "large result".repeat(6_000),
        })],
        structured_content: None,
        is_error: Some(true),
        meta: None,
    }));
    let event = match &oversized {
        RolloutItem::EventMsg(event) => event,
        _ => unreachable!(),
    };
    assert!(!is_persisted_rollout_item(
        &oversized,
        ThreadHistoryMode::Legacy
    ));
    assert!(!should_persist_event_msg(event, ThreadHistoryMode::Legacy));
    assert!(persisted_rollout_item(&oversized, ThreadHistoryMode::Legacy).is_none());
}
