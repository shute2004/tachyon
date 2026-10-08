use super::PreviousSectionState;
use super::WorldStateContextFragment;
use super::WorldStateSection;
#[path = "tools_budget.rs"]
mod tools_budget;

use crate::context::ContextualUserFragment;
use codex_extension_api::RenderedWorldStateFragment;
use codex_protocol::models::ContentItemKind;
use codex_protocol::protocol::TOOLS_CLOSE_TAG;
use codex_protocol::protocol::TOOLS_OPEN_TAG;
use std::collections::BTreeMap;

const MAX_RENDERED_FRAGMENT_BYTES: usize = 4 * 1024;
const MAX_NAMESPACE_DESCRIPTION_CHARS: usize = 250;

/// Deferred tool namespaces visible to the model for one sampling step.
#[derive(Debug, Default)]
pub(crate) struct ToolsState {
    deferred_namespaces: BTreeMap<String, String>,
}

impl ToolsState {
    pub(crate) fn new(deferred_namespaces: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            deferred_namespaces: deferred_namespaces
                .into_iter()
                .map(|(namespace, description)| {
                    let first_line = description.lines().next().unwrap_or_default().trim();
                    let mut description: String = first_line
                        .chars()
                        .take(MAX_NAMESPACE_DESCRIPTION_CHARS + 1)
                        .collect();
                    if description.chars().count() > MAX_NAMESPACE_DESCRIPTION_CHARS {
                        description = description
                            .chars()
                            .take(MAX_NAMESPACE_DESCRIPTION_CHARS - 3)
                            .collect();
                        description.push_str("...");
                    }
                    (namespace, description)
                })
                .collect(),
        }
    }
}

impl WorldStateSection for ToolsState {
    const ID: &'static str = "tools";
    // Object-valued entries let RFC 7386 patches add and remove namespaces individually.
    type Snapshot = BTreeMap<String, String>;

    fn snapshot(&self) -> Self::Snapshot {
        self.deferred_namespaces.clone()
    }

    fn should_persist(&self) -> bool {
        !self.deferred_namespaces.is_empty()
    }

    fn render_diff(
        &self,
        previous: PreviousSectionState<'_, Self::Snapshot>,
    ) -> Option<Box<dyn ContextualUserFragment>> {
        let current = self.snapshot();
        if matches!(previous, PreviousSectionState::Known(previous) if previous == &current)
            || self.deferred_namespaces.is_empty()
                && matches!(
                    previous,
                    PreviousSectionState::Absent | PreviousSectionState::Unknown
                )
        {
            return None;
        }

        let body_budget = MAX_RENDERED_FRAGMENT_BYTES
            .saturating_sub(TOOLS_OPEN_TAG.len() + TOOLS_CLOSE_TAG.len());
        let body = match previous {
            PreviousSectionState::Absent | PreviousSectionState::Unknown => {
                tools_budget::render_namespace_groups(
                    &[("Deferred tool namespaces", &self.deferred_namespaces)],
                    self.deferred_namespaces.is_empty(),
                    body_budget,
                )
            }
            PreviousSectionState::Known(previous) => {
                let added = self
                    .deferred_namespaces
                    .iter()
                    .filter(|(namespace, description)| {
                        previous.get(*namespace) != Some(*description)
                    })
                    .map(|(namespace, description)| (namespace.clone(), description.clone()))
                    .collect();
                let removed = previous
                    .iter()
                    .filter(|(namespace, _)| !self.deferred_namespaces.contains_key(*namespace))
                    .map(|(namespace, description)| (namespace.clone(), description.clone()))
                    .collect();
                tools_budget::render_namespace_groups(
                    &[
                        ("Added deferred tool namespaces", &added),
                        ("Removed deferred tool namespaces", &removed),
                    ],
                    self.deferred_namespaces.is_empty(),
                    body_budget,
                )
            }
        };
        Some(Box::new(WorldStateContextFragment {
            fragment: RenderedWorldStateFragment::new(
                "developer",
                (TOOLS_OPEN_TAG, TOOLS_CLOSE_TAG),
                body,
            ),
            content_kind: ContentItemKind("tools.deferred_namespaces".to_string()),
        }))
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
