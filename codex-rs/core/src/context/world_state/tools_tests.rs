use super::MAX_NAMESPACE_DESCRIPTION_CHARS;
use super::MAX_RENDERED_FRAGMENT_BYTES;
use super::ToolsState;
use crate::context::world_state::PreviousSectionState;
use crate::context::world_state::WorldState;
use crate::context::world_state::WorldStateSection;
use crate::context::world_state::WorldStateSnapshot;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[test]
fn renders_first_line_of_namespace_descriptions() {
    let tools = ToolsState::new([
        (
            "app".to_string(),
            "  control the Codex App  \nAdditional instructions.".to_string(),
        ),
        (
            "gmail".to_string(),
            "access your Google Gmail Account & labels".to_string(),
        ),
        ("hotline".to_string(), String::new()),
    ]);

    let rendered = tools
        .render_diff(PreviousSectionState::Absent)
        .expect("tools state should render")
        .render();

    assert_eq!(
        rendered,
        "<tools>\nDeferred tool namespaces:\n- app: control the Codex App\n- gmail: access your Google Gmail Account &amp; labels\n- hotline\n</tools>"
    );
}

#[test]
fn renders_added_removed_and_updated_namespace_descriptions() {
    let tools = ToolsState::new([
        ("app".to_string(), "control the Codex App".to_string()),
        (
            "gmail".to_string(),
            "access your Google Gmail Account".to_string(),
        ),
    ]);
    let previous = BTreeMap::from([
        ("gmail".to_string(), "old Gmail description".to_string()),
        (
            "hotline".to_string(),
            "access hotline information".to_string(),
        ),
    ]);

    let rendered = tools
        .render_diff(PreviousSectionState::Known(&previous))
        .expect("tools state delta should render")
        .render();

    assert_eq!(
        rendered,
        "<tools>\nAdded deferred tool namespaces:\n- app: control the Codex App\n- gmail: access your Google Gmail Account\nRemoved deferred tool namespaces:\n- hotline: access hotline information\n</tools>"
    );
}

#[test]
fn descriptions_at_250_chars_are_unchanged_and_longer_values_use_ellipsis() {
    let ascii_at_limit = "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS);
    let unicode_at_limit = "🦀".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS);
    let ascii_over_limit = "b".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS + 1);
    let unicode_over_limit = "🦀".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS + 1);
    let tools = ToolsState::new([
        ("ascii_250".to_string(), ascii_at_limit.clone()),
        ("unicode_250".to_string(), unicode_at_limit.clone()),
        ("ascii_251".to_string(), ascii_over_limit),
        ("unicode_251".to_string(), unicode_over_limit),
    ]);

    assert_eq!(
        tools.snapshot(),
        BTreeMap::from([
            ("ascii_250".to_string(), ascii_at_limit),
            ("unicode_250".to_string(), unicode_at_limit),
            (
                "ascii_251".to_string(),
                format!("{}...", "b".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS - 3))
            ),
            (
                "unicode_251".to_string(),
                format!("{}...", "🦀".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS - 3))
            ),
        ])
    );
}

#[test]
fn description_normalization_keeps_trimmed_first_line_semantics() {
    let tools = ToolsState::new([
        (
            "trimmed".to_string(),
            " \t first line \t\r\nsecond line".to_string(),
        ),
        ("empty_first_line".to_string(), "\nsecond line".to_string()),
        (
            "whitespace_first_line".to_string(),
            " \t\nsecond line".to_string(),
        ),
    ]);

    assert_eq!(
        tools.snapshot(),
        BTreeMap::from([
            ("trimmed".to_string(), "first line".to_string()),
            ("empty_first_line".to_string(), String::new()),
            ("whitespace_first_line".to_string(), String::new()),
        ])
    );
}

#[test]
fn normalized_tools_snapshot_round_trips_as_known_resumed_state() {
    let raw_description = "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS + 1);
    let normalized_description = format!("{}...", "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS - 3));
    let mut original = WorldState::default();
    original.add_section(ToolsState::new([(
        "app".to_string(),
        raw_description.clone(),
    )]));

    let snapshot_json =
        serde_json::to_value(original.snapshot()).expect("serialize the world-state snapshot");
    assert_eq!(
        snapshot_json,
        serde_json::json!({"tools": {"app": normalized_description.clone()}})
    );
    let resumed_snapshot: WorldStateSnapshot =
        serde_json::from_value(snapshot_json).expect("deserialize a resumed world-state snapshot");
    let mut resumed = WorldState::default();
    resumed.add_section(ToolsState::new([("app".to_string(), raw_description)]));

    assert!(resumed.render_diff(&resumed_snapshot).is_empty());
}

#[test]
fn legacy_250_char_snapshot_gets_one_ellipsis_delta_then_noops() {
    let legacy_description = "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS);
    let normalized_description = format!("{}...", "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS - 3));
    let legacy_snapshot: WorldStateSnapshot = serde_json::from_value(serde_json::json!({
        "tools": {"app": legacy_description}
    }))
    .expect("deserialize the legacy world-state snapshot");
    let mut current = WorldState::default();
    current.add_section(ToolsState::new([(
        "app".to_string(),
        "a".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS + 1),
    )]));

    let delta = current.render_diff(&legacy_snapshot);
    assert_eq!(delta.len(), 1);
    assert_eq!(
        delta[0].render(),
        format!(
            "<tools>\nAdded deferred tool namespaces:\n- app: {normalized_description}\n</tools>"
        )
    );

    let current_snapshot = current.snapshot();
    assert!(current.render_diff(&current_snapshot).is_empty());
}

#[test]
fn rendering_does_not_change_normalized_tool_snapshot() {
    let tools = ToolsState::new((0..100).map(|index| {
        (
            format!("namespace_{index}"),
            format!("{}tail", "🦀".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS)),
        )
    }));
    let mut world_state = WorldState::default();
    world_state.add_section(tools);
    let snapshot = world_state.snapshot();

    let rendered = world_state.render_full();
    assert_eq!(rendered.len(), 1);
    assert!(rendered[0].render().len() <= MAX_RENDERED_FRAGMENT_BYTES);
    assert_eq!(world_state.snapshot(), snapshot);
    assert_eq!(
        serde_json::to_value(&snapshot).expect("serialize normalized snapshot")["tools"]["namespace_0"],
        serde_json::json!(format!(
            "{}...",
            "🦀".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS - 3)
        ))
    );
}

#[test]
fn caps_rendered_tools_fragment_after_xml_escaping() {
    let tools = ToolsState::new((0..100).map(|index| {
        (
            format!("namespace_{index}"),
            "&".repeat(MAX_NAMESPACE_DESCRIPTION_CHARS),
        )
    }));

    let rendered = tools
        .render_diff(PreviousSectionState::Absent)
        .expect("tools state should render")
        .render();

    assert!(rendered.len() <= MAX_RENDERED_FRAGMENT_BYTES);
    assert!(rendered.contains("- namespace_0: &amp;"));
    assert!(rendered.contains("&amp;..."));
}
