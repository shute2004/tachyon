use super::*;
use pretty_assertions::assert_eq;

fn parse_toml(value: &str) -> TomlValue {
    toml::from_str(value).expect("TOML should parse")
}

#[test]
fn normalizes_aliases_from_root_and_matching_subpath() {
    let value = parse_toml(
        r#"
[agents]
max_threads = 4

[memories]
no_memories_if_mcp_or_web_search = true
"#,
    );
    let expected = parse_toml(
        r#"
[agents]
max_concurrent_threads_per_session = 4

[memories]
disable_on_external_context = true
"#,
    );

    assert_eq!(normalized_with_key_aliases(&value, &[]), expected);

    let agents = parse_toml("max_threads = 7");
    let agents_path = vec!["agents".to_string()];
    assert_eq!(
        normalized_with_key_aliases(&agents, &agents_path),
        parse_toml("max_concurrent_threads_per_session = 7")
    );
}

#[test]
fn canonical_key_wins_and_legacy_key_is_removed() {
    let value = parse_toml(
        r#"
disable_on_external_context = true
no_memories_if_mcp_or_web_search = false
"#,
    );

    assert_eq!(
        normalized_with_key_aliases(&value, &["memories".to_string()]),
        parse_toml("disable_on_external_context = true")
    );
}

#[test]
fn unrelated_tables_and_arrays_keep_lookalike_keys() {
    let value = parse_toml(
        r#"
[unrelated]
max_threads = 4
no_memories_if_mcp_or_web_search = true

[[items]]
max_threads = 5

[[items]]
no_memories_if_mcp_or_web_search = false
"#,
    );

    assert_eq!(normalized_with_key_aliases(&value, &[]), value);
}

#[test]
fn nested_arrays_at_matching_path_keep_alias_normalization() {
    let value = parse_toml(
        r#"
[[agents]]
max_threads = 4

[[agents]]
max_threads = 6
"#,
    );
    let expected = parse_toml(
        r#"
[[agents]]
max_concurrent_threads_per_session = 4

[[agents]]
max_concurrent_threads_per_session = 6
"#,
    );

    assert_eq!(normalized_with_key_aliases(&value, &[]), expected);
}

#[test]
fn nonmatching_and_longer_paths_return_the_value_unchanged() {
    let value = parse_toml(
        r#"
max_threads = 4
no_memories_if_mcp_or_web_search = true
"#,
    );

    assert_eq!(
        normalized_with_key_aliases(&value, &["agent".to_string()]),
        value
    );
    assert_eq!(
        normalized_with_key_aliases(&value, &["agents".to_string(), "nested".to_string()]),
        value
    );
}
