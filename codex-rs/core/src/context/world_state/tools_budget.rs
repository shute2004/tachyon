use crate::context::environment_context::push_xml_escaped_text;
use std::collections::BTreeMap;

const OMITTED_LINE_RESERVE_BYTES: usize = 64;
const EMPTY_STATE_LINE: &str = "No deferred tool namespaces remain.\n";
const DESCRIPTION_SEPARATOR: &str = ": ";
const DESCRIPTION_ELLIPSIS: &str = "...";

struct BudgetedGroup<'a> {
    label: &'static str,
    entries: Vec<BudgetedEntry<'a>>,
    omitted: usize,
}

struct BudgetedEntry<'a> {
    escaped_name: String,
    description: &'a str,
    escaped_description: String,
    description_boundaries: Vec<usize>,
    description_prefix_chars: usize,
    selected: bool,
}

pub(super) fn render_namespace_groups(
    groups: &[(&'static str, &BTreeMap<String, String>)],
    current_is_empty: bool,
    body_budget: usize,
) -> String {
    let mut groups: Vec<BudgetedGroup<'_>> = groups
        .iter()
        .map(|(label, namespaces)| BudgetedGroup {
            label,
            entries: namespaces
                .iter()
                .map(|(namespace, description)| BudgetedEntry {
                    escaped_name: xml_escape(namespace),
                    description,
                    escaped_description: String::new(),
                    description_boundaries: Vec::new(),
                    description_prefix_chars: 0,
                    selected: false,
                })
                .collect(),
            omitted: 0,
        })
        .collect();

    let fixed_body_bytes = fixed_body_bytes(&groups, current_is_empty);
    let all_names_bytes = groups
        .iter()
        .flat_map(|group| &group.entries)
        .map(name_line_bytes)
        .fold(0usize, usize::saturating_add);

    if fixed_body_bytes.saturating_add(all_names_bytes) <= body_budget {
        for group in &mut groups {
            for entry in &mut group.entries {
                entry.selected = true;
                (entry.escaped_description, entry.description_boundaries) =
                    xml_escape_with_boundaries(entry.description);
            }
        }

        let mut remaining_description_bytes = body_budget - fixed_body_bytes - all_names_bytes;
        let all_descriptions_bytes = groups
            .iter()
            .flat_map(|group| &group.entries)
            .map(full_description_bytes)
            .fold(0usize, usize::saturating_add);
        if all_descriptions_bytes <= remaining_description_bytes {
            for entry in groups.iter_mut().flat_map(|group| &mut group.entries) {
                entry.description_prefix_chars = entry.description_boundaries.len();
            }
        } else {
            allocate_description_prefixes(&mut groups, &mut remaining_description_bytes);
        }
    } else {
        let omission_reserve = groups
            .iter()
            .filter(|group| !group.entries.is_empty())
            .count()
            .saturating_mul(OMITTED_LINE_RESERVE_BYTES);
        let mut remaining_name_bytes = body_budget
            .saturating_sub(fixed_body_bytes)
            .saturating_sub(omission_reserve);
        for group in &mut groups {
            for entry in &mut group.entries {
                let entry_bytes = name_line_bytes(entry);
                if entry_bytes <= remaining_name_bytes {
                    entry.selected = true;
                    remaining_name_bytes -= entry_bytes;
                } else {
                    group.omitted += 1;
                }
            }
        }
    }

    render_body(&groups, current_is_empty, body_budget)
}

fn fixed_body_bytes(groups: &[BudgetedGroup<'_>], current_is_empty: bool) -> usize {
    let heading_bytes = groups
        .iter()
        .filter(|group| !group.entries.is_empty())
        .map(|group| group.label.len() + ":\n".len())
        .fold(0usize, usize::saturating_add);
    1usize
        .saturating_add(heading_bytes)
        .saturating_add(if current_is_empty {
            EMPTY_STATE_LINE.len()
        } else {
            0
        })
}

fn name_line_bytes(entry: &BudgetedEntry<'_>) -> usize {
    "- ".len() + entry.escaped_name.len() + "\n".len()
}

fn full_description_bytes(entry: &BudgetedEntry<'_>) -> usize {
    if entry.description_boundaries.is_empty() {
        0
    } else {
        DESCRIPTION_SEPARATOR.len() + entry.escaped_description.len()
    }
}

fn allocate_description_prefixes(groups: &mut [BudgetedGroup<'_>], remaining_bytes: &mut usize) {
    loop {
        let mut allocated_any = false;
        for entry in groups.iter_mut().flat_map(|group| &mut group.entries) {
            if entry.description_prefix_chars == entry.description_boundaries.len() {
                continue;
            }

            let current_cost = selected_description_bytes(entry);
            let next_prefix_chars = entry.description_prefix_chars + 1;
            let next_cost = selected_description_bytes_for_prefix(entry, next_prefix_chars);
            let full_cost = full_description_bytes(entry);
            let full_additional_cost = full_cost.saturating_sub(current_cost);
            if full_cost <= next_cost
                && (full_cost <= current_cost || full_additional_cost <= *remaining_bytes)
            {
                entry.description_prefix_chars = entry.description_boundaries.len();
                if full_cost > current_cost {
                    *remaining_bytes -= full_additional_cost;
                } else {
                    *remaining_bytes += current_cost - full_cost;
                }
                allocated_any = true;
                continue;
            }
            let additional_cost = next_cost.saturating_sub(current_cost);
            if next_cost <= current_cost || additional_cost <= *remaining_bytes {
                if next_cost > current_cost {
                    *remaining_bytes -= additional_cost;
                } else {
                    *remaining_bytes += current_cost - next_cost;
                }
                entry.description_prefix_chars = next_prefix_chars;
                allocated_any = true;
            }
        }

        if !allocated_any {
            break;
        }
    }
}

fn selected_description_bytes(entry: &BudgetedEntry<'_>) -> usize {
    selected_description_bytes_for_prefix(entry, entry.description_prefix_chars)
}

fn selected_description_bytes_for_prefix(entry: &BudgetedEntry<'_>, prefix_chars: usize) -> usize {
    if prefix_chars == 0 {
        return 0;
    }
    let prefix_end = entry.description_boundaries[prefix_chars - 1];
    let suffix_bytes = if prefix_chars < entry.description_boundaries.len() {
        DESCRIPTION_ELLIPSIS.len()
    } else {
        0
    };
    DESCRIPTION_SEPARATOR.len() + prefix_end + suffix_bytes
}

fn render_body(groups: &[BudgetedGroup<'_>], current_is_empty: bool, body_budget: usize) -> String {
    let mut rendered = String::new();
    rendered.push('\n');
    for group in groups {
        if group.entries.is_empty() {
            continue;
        }
        rendered.push_str(group.label);
        rendered.push_str(":\n");
        for entry in &group.entries {
            if !entry.selected {
                continue;
            }
            rendered.push_str("- ");
            rendered.push_str(&entry.escaped_name);
            if entry.description_prefix_chars > 0 {
                let prefix_end = entry.description_boundaries[entry.description_prefix_chars - 1];
                rendered.push_str(DESCRIPTION_SEPARATOR);
                rendered.push_str(&entry.escaped_description[..prefix_end]);
                if entry.description_prefix_chars < entry.description_boundaries.len() {
                    rendered.push_str(DESCRIPTION_ELLIPSIS);
                }
            }
            rendered.push('\n');
        }
        if group.omitted > 0 {
            let omission_line = format!("... {} additional namespaces omitted.\n", group.omitted);
            debug_assert!(omission_line.len() <= OMITTED_LINE_RESERVE_BYTES);
            rendered.push_str(&omission_line);
        }
    }
    if current_is_empty {
        rendered.push_str(EMPTY_STATE_LINE);
    }
    debug_assert!(rendered.len() <= body_budget);
    rendered
}

fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    push_xml_escaped_text(&mut escaped, text);
    escaped
}

fn xml_escape_with_boundaries(text: &str) -> (String, Vec<usize>) {
    let mut escaped = String::with_capacity(text.len());
    let mut boundaries = Vec::with_capacity(text.chars().count());
    for character in text.chars() {
        let mut buffer = [0; 4];
        push_xml_escaped_text(&mut escaped, character.encode_utf8(&mut buffer));
        boundaries.push(escaped.len());
    }
    (escaped, boundaries)
}

#[cfg(test)]
#[path = "tools_budget_tests.rs"]
mod tests;
