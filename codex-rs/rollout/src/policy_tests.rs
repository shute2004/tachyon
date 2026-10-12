use std::borrow::Cow;

use codex_protocol::ThreadId;
use codex_protocol::items::CommandExecutionItem;
use codex_protocol::items::CommandExecutionStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecCommandSource;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ThreadHistoryMode;
use pretty_assertions::assert_eq;

use super::COMMAND_OUTPUT_TRUNCATION_MARKER;
use super::PERSISTED_COMMAND_OUTPUT_MAX_BYTES;
use super::into_persisted_rollout_items;
use super::is_persisted_rollout_item;
use super::persisted_rollout_item;
use super::should_persist_event_msg;
use crate::RolloutItem;

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
