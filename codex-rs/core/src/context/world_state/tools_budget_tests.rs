use super::render_namespace_groups;
use crate::context::world_state::PreviousSectionState;
use crate::context::world_state::WorldStateSection;
use crate::context::world_state::tools::ToolsState;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

const SINGLE_GROUP_LABEL: &str = "Deferred tool namespaces";

fn fixed_single_group_body_bytes() -> usize {
    1 + SINGLE_GROUP_LABEL.len() + ":\n".len()
}

fn names_only_body_bytes(names: &BTreeMap<String, String>) -> usize {
    fixed_single_group_body_bytes()
        + names
            .keys()
            .map(|name| "- ".len() + name.len() + "\n".len())
            .sum::<usize>()
}

fn render_single_group(names: &BTreeMap<String, String>, body_budget: usize) -> String {
    render_namespace_groups(&[(SINGLE_GROUP_LABEL, names)], false, body_budget)
}

#[test]
fn preserves_full_fragment_at_4096_and_4095_bytes_including_tags() {
    let fixed_fragment_bytes = codex_protocol::protocol::TOOLS_OPEN_TAG.len()
        + codex_protocol::protocol::TOOLS_CLOSE_TAG.len()
        + fixed_single_group_body_bytes()
        + 3;
    let name_length = super::super::MAX_RENDERED_FRAGMENT_BYTES - fixed_fragment_bytes;
    let exact_name = "n".repeat(name_length);
    let below_name = "n".repeat(name_length - 1);

    for (name, expected_bytes) in [(exact_name, 4096), (below_name, 4095)] {
        let tools = ToolsState::new([(name, String::new())]);
        let rendered = tools
            .render_diff(PreviousSectionState::Absent)
            .expect("tools state should render")
            .render();

        assert_eq!(rendered.len(), expected_bytes);
        assert!(!rendered.contains("additional namespaces omitted."));
    }
}

#[test]
fn preserves_short_full_escaped_names_and_descriptions() {
    let tools = ToolsState::new([("a&<>\"'🦀".to_string(), "d&<>\"'🦀".to_string())]);

    let rendered = tools
        .render_diff(PreviousSectionState::Absent)
        .expect("tools state should render")
        .render();

    assert_eq!(
        rendered,
        "<tools>\nDeferred tool namespaces:\n- a&amp;&lt;&gt;&quot;&apos;🦀: d&amp;&lt;&gt;&quot;&apos;🦀\n</tools>"
    );
}

#[test]
fn retains_all_names_before_distributing_long_escaped_descriptions() {
    let namespaces =
        ToolsState::new((0..100).map(|index| (format!("namespace_{index:03}"), "&".repeat(300))));
    let rendered = namespaces
        .render_diff(PreviousSectionState::Absent)
        .expect("initial tools state should render")
        .render();

    assert!(rendered.len() <= super::super::MAX_RENDERED_FRAGMENT_BYTES);
    for index in 0..100 {
        assert!(rendered.contains(&format!("- namespace_{index:03}:")));
    }
    assert!(rendered.contains("&amp;..."));
}

#[test]
fn keeps_combined_added_removed_and_empty_notice_fragments_bounded() {
    let current = (0..40)
        .map(|index| (format!("added_{index:03}"), "&".repeat(250)))
        .collect::<BTreeMap<_, _>>();
    let mut previous = current
        .keys()
        .map(|name| (name.clone(), "old description".to_string()))
        .collect::<BTreeMap<_, _>>();
    previous.extend((0..40).map(|index| (format!("removed_{index:03}"), "old".to_string())));
    let tools = ToolsState::new(current);
    let rendered = tools
        .render_diff(PreviousSectionState::Known(&previous))
        .expect("changed tools state should render")
        .render();

    assert!(rendered.len() <= super::super::MAX_RENDERED_FRAGMENT_BYTES);
    assert!(rendered.contains("Added deferred tool namespaces:\n"));
    assert!(rendered.contains("Removed deferred tool namespaces:\n"));
    for index in 0..40 {
        assert!(rendered.contains(&format!("- added_{index:03}:")));
        assert!(rendered.contains(&format!("- removed_{index:03}")));
    }

    let empty = ToolsState::default();
    let removed = (0..100)
        .map(|index| (format!("removed_{index:03}"), "old".to_string()))
        .collect::<BTreeMap<_, _>>();
    let rendered = empty
        .render_diff(PreviousSectionState::Known(&removed))
        .expect("removed tools and empty notice should render")
        .render();
    assert!(rendered.len() <= super::super::MAX_RENDERED_FRAGMENT_BYTES);
    assert!(rendered.contains("Removed deferred tool namespaces:\n"));
    assert!(rendered.contains("No deferred tool namespaces remain.\n"));
}

#[test]
fn oversized_early_name_does_not_hide_later_small_names() {
    let namespaces = BTreeMap::from([
        ("a".repeat(1_000), "unused".to_string()),
        ("z-small".to_string(), "unused".to_string()),
    ]);

    let rendered = render_single_group(&namespaces, 512);

    assert!(rendered.contains("- z-small\n"));
    assert_eq!(
        rendered
            .matches("... 1 additional namespaces omitted.\n")
            .count(),
        1
    );
}

#[test]
fn omission_counts_are_exact_per_group_and_group_order_is_stable() {
    let added = BTreeMap::from([
        ("a".repeat(1_000), String::new()),
        ("z-added".to_string(), String::new()),
    ]);
    let removed = BTreeMap::from([
        ("b".repeat(1_000), String::new()),
        ("z-removed".to_string(), String::new()),
    ]);

    let rendered = render_namespace_groups(
        &[
            ("Added deferred tool namespaces", &added),
            ("Removed deferred tool namespaces", &removed),
        ],
        false,
        512,
    );

    assert!(
        rendered.find("Added deferred tool namespaces:").unwrap()
            < rendered.find("Removed deferred tool namespaces:").unwrap()
    );
    assert!(rendered.contains("- z-added\n"));
    assert!(rendered.contains("- z-removed\n"));
    assert_eq!(
        rendered
            .matches("... 1 additional namespaces omitted.\n")
            .count(),
        2
    );
}

#[test]
fn honors_exact_name_budget_and_omits_when_one_byte_short() {
    let exact_body_budget = 512;
    let name_length = exact_body_budget - fixed_single_group_body_bytes() - 3;
    let name = "n".repeat(name_length);
    let namespaces = BTreeMap::from([(name.clone(), String::new())]);

    let exact = render_single_group(&namespaces, exact_body_budget);
    assert_eq!(exact.len(), exact_body_budget);
    assert!(exact.contains(&format!("- {name}\n")));

    let one_byte_short = render_single_group(&namespaces, exact_body_budget - 1);
    assert!(one_byte_short.len() <= exact_body_budget - 1);
    assert!(!one_byte_short.contains(&format!("- {name}\n")));
    assert!(one_byte_short.contains("... 1 additional namespaces omitted.\n"));
}

#[test]
fn escaped_entities_and_unicode_scalars_are_never_split() {
    for (description, description_budget, expected_prefix) in [
        ("&".to_string() + &"x".repeat(30), 10, "&amp;..."),
        ("🦀".to_string() + &"x".repeat(30), 9, "🦀..."),
    ] {
        let namespaces = BTreeMap::from([("a".to_string(), description)]);
        let body_budget = names_only_body_bytes(&namespaces) + description_budget;
        let rendered = render_single_group(&namespaces, body_budget);

        assert!(rendered.contains(&format!("- a: {expected_prefix}\n")));
        assert!(rendered.len() <= body_budget);
    }
}

#[test]
fn distributes_description_prefixes_fairly_in_stable_order() {
    let namespaces = BTreeMap::from([
        ("a".to_string(), "abcdefghij".to_string()),
        ("b".to_string(), "abcdefghij".to_string()),
    ]);
    let body_budget = names_only_body_bytes(&namespaces) + 20;

    let rendered = render_single_group(&namespaces, body_budget);
    let first = rendered
        .lines()
        .find(|line| line.starts_with("- a: "))
        .expect("first namespace should render");
    let second = rendered
        .lines()
        .find(|line| line.starts_with("- b: "))
        .expect("second namespace should render");

    assert!(first.ends_with("..."));
    assert!(second.ends_with("..."));
    assert_eq!(first.strip_prefix("- a: ").unwrap(), "abcde...");
    assert_eq!(second.strip_prefix("- b: ").unwrap(), "abcde...");
}

#[test]
fn uses_complete_short_descriptions_even_when_global_budget_truncates_other_rows() {
    for description in ["ab", "abc"] {
        let namespaces = BTreeMap::from([
            ("a".to_string(), description.to_string()),
            ("z".to_string(), "a much longer description".to_string()),
        ]);
        let remaining_description_bytes = ": ".len() + description.len();
        let body_budget = names_only_body_bytes(&namespaces) + remaining_description_bytes;

        let rendered = render_single_group(&namespaces, body_budget);

        assert!(rendered.contains(&format!("- a: {description}\n")));
        assert!(!rendered.contains(&format!("- a: {description}...\n")));
        assert!(rendered.contains("- z\n"));
        assert!(rendered.len() <= body_budget);
    }
}

#[test]
fn completes_a_prefix_when_full_text_costs_no_more_than_next_truncated_prefix() {
    let namespaces = BTreeMap::from([
        ("a".to_string(), "abcdef".to_string()),
        ("b".to_string(), "abcdefghijklmnopqrstuvwxyz".to_string()),
    ]);
    let body_budget = names_only_body_bytes(&namespaces) + 16;

    let rendered = render_single_group(&namespaces, body_budget);

    assert!(rendered.contains("- a: abcdef\n"));
    assert!(!rendered.contains("- a: abcdef...\n"));
    assert!(rendered.contains("- b: abc...\n"));
    assert!(rendered.len() <= body_budget);
}

#[test]
fn full_fit_description_uses_exact_output_without_unused_ellipsis() {
    let namespaces = BTreeMap::from([("app".to_string(), "short".to_string())]);
    let body_budget = names_only_body_bytes(&namespaces) + ": ".len() + "short".len();

    assert_eq!(
        render_single_group(&namespaces, body_budget),
        "\nDeferred tool namespaces:\n- app: short\n"
    );
}

#[test]
fn budgeted_rendering_does_not_change_snapshot_or_equal_snapshot_diff() {
    let tools =
        ToolsState::new((0..100).map(|index| (format!("namespace_{index:03}"), "&".repeat(300))));
    let snapshot = tools.snapshot();

    let _ = tools
        .render_diff(PreviousSectionState::Absent)
        .expect("initial state should render")
        .render();

    assert_eq!(tools.snapshot(), snapshot);
    assert!(
        tools
            .render_diff(PreviousSectionState::Known(&snapshot))
            .is_none()
    );
}
