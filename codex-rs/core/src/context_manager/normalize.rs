use codex_context_fragments::set_annotated_content;
use codex_context_fragments::to_annotated_content;
use codex_history::ResponseItemEnvelope;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::InputModality;
use std::collections::HashSet;
use uuid::Uuid;

use crate::context::ContextualUserFragment;
use crate::context::UnsupportedMedia;
use crate::context_manager::history_item::HistoryToolSide;
use crate::context_manager::history_item::ResponsesToolPairingClass;
use crate::context_manager::history_item::tool_correlation;
use crate::util::error_or_panic;
use tracing::info;

// Changing this value would change model-visible IDs and invalidate prompt caches.
const SYNTHETIC_OUTPUT_ID_NAMESPACE: Uuid = Uuid::from_u128(0x90d38d3e_6a5b_4d52_bfe2_2f1e634bfac4);

pub(crate) fn ensure_call_outputs_present(items: &mut Vec<ResponseItemEnvelope>) {
    let output_keys = items
        .iter()
        .filter_map(|envelope| {
            let correlation = tool_correlation(&envelope.item)?;
            (correlation.side == HistoryToolSide::Output)
                .then_some((correlation.compatibility_pairing_class, correlation.call_id))
        })
        .collect::<HashSet<_>>();

    // Collect synthetic outputs to insert immediately after their calls.
    // Store the insertion position (index of call) alongside the item so
    // we can insert in reverse order and avoid index shifting.
    let mut missing_outputs_to_insert: Vec<(usize, ResponseItemEnvelope)> = Vec::new();

    for (idx, envelope) in items.iter().enumerate() {
        let Some(correlation) = tool_correlation(&envelope.item) else {
            continue;
        };
        if correlation.side != HistoryToolSide::Call
            || output_keys.contains(&(correlation.compatibility_pairing_class, correlation.call_id))
        {
            continue;
        }

        // Construction remains in the Responses compatibility layer for now. Matching above uses
        // generic call/result correlation facts plus an explicitly compatibility-only Responses
        // pairing class, so the latter cannot be mistaken for the future canonical history taxonomy.
        match &envelope.item {
            ResponseItem::FunctionCall { id, call_id, .. } => {
                info!("Function call output is missing for call id: {call_id}");
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItemEnvelope::new(ResponseItem::FunctionCallOutput {
                        id: synthetic_output_id("fco", id.as_deref()),
                        call_id: Some(call_id.clone()),
                        name: None,
                        namespace: None,
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    }),
                ));
            }
            ResponseItem::ToolSearchCall {
                id,
                call_id: Some(call_id),
                ..
            } => {
                info!("Tool search output is missing for call id: {call_id}");
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItemEnvelope::new(ResponseItem::ToolSearchOutput {
                        id: synthetic_output_id("tso", id.as_deref()),
                        call_id: Some(call_id.clone()),
                        status: "completed".to_string(),
                        execution: "client".to_string(),
                        tools: Vec::new(),
                        internal_chat_message_metadata_passthrough: None,
                    }),
                ));
            }
            ResponseItem::CustomToolCall { id, call_id, .. } => {
                error_or_panic(format!(
                    "Custom tool call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItemEnvelope::new(ResponseItem::CustomToolCallOutput {
                        id: synthetic_output_id("ctco", id.as_deref()),
                        call_id: call_id.clone(),
                        name: None,
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    }),
                ));
            }
            // LocalShellCall is represented in upstream streams by a FunctionCallOutput.
            ResponseItem::LocalShellCall {
                id,
                call_id: Some(call_id),
                ..
            } => {
                error_or_panic(format!(
                    "Local shell call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItemEnvelope::new(ResponseItem::FunctionCallOutput {
                        id: synthetic_output_id("fco", id.as_deref()),
                        call_id: Some(call_id.clone()),
                        name: None,
                        namespace: None,
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        internal_chat_message_metadata_passthrough: None,
                    }),
                ));
            }
            _ => {}
        }
    }
    drop(output_keys);

    // Insert synthetic outputs in reverse index order to avoid re-indexing.
    for (idx, output_item) in missing_outputs_to_insert.into_iter().rev() {
        items.insert(idx + 1, output_item);
    }
}

/// Derives a stable ID for a prompt-only output from its source call's item ID.
///
/// Prompt normalization can run repeatedly without persisting its synthetic
/// outputs, so the namespace and name format must remain stable across retries
/// and resumes to preserve prompt-cache reuse. Returning `None` when the source
/// call has no ID preserves the legacy behavior for older history items.
fn synthetic_output_id(prefix: &str, item_id: Option<&str>) -> Option<ResponseItemId> {
    let source_id = item_id.filter(|id| !id.is_empty())?;
    let name = format!("{prefix}:{source_id}");
    Some(ResponseItemId::with_suffix(
        prefix,
        Uuid::new_v5(&SYNTHETIC_OUTPUT_ID_NAMESPACE, name.as_bytes()),
    ))
}

pub(crate) fn remove_orphan_outputs(items: &mut Vec<ResponseItemEnvelope>) {
    let call_keys = items
        .iter()
        .filter_map(|envelope| {
            let correlation = tool_correlation(&envelope.item)?;
            (correlation.side == HistoryToolSide::Call)
                .then_some((correlation.compatibility_pairing_class, correlation.call_id))
        })
        .collect::<HashSet<_>>();

    let mut orphan_positions = Vec::new();
    for (position, envelope) in items.iter().enumerate() {
        let Some(correlation) = tool_correlation(&envelope.item) else {
            continue;
        };
        if correlation.side != HistoryToolSide::Output
            || !correlation.local_counterpart_required
            || call_keys.contains(&(correlation.compatibility_pairing_class, correlation.call_id))
        {
            continue;
        }

        match correlation.compatibility_pairing_class {
            ResponsesToolPairingClass::FunctionCallOutput => error_or_panic(format!(
                "Orphan function call output for call id: {}",
                correlation.call_id
            )),
            ResponsesToolPairingClass::CustomToolCallOutput => error_or_panic(format!(
                "Orphan custom tool call output for call id: {}",
                correlation.call_id
            )),
            ResponsesToolPairingClass::ToolSearchOutput => error_or_panic(format!(
                "Orphan tool search output for call id: {}",
                correlation.call_id
            )),
        }
        orphan_positions.push(position);
    }
    drop(call_keys);

    if !orphan_positions.is_empty() {
        let mut orphan_positions = orphan_positions.into_iter().peekable();
        let mut position = 0;
        items.retain(|_| {
            let retain = orphan_positions.peek() != Some(&position);
            if !retain {
                orphan_positions.next();
            }
            position += 1;
            retain
        });
    }
}

pub(crate) fn remove_corresponding_for(items: &mut Vec<ResponseItemEnvelope>, item: &ResponseItem) {
    let Some(correlation) = tool_correlation(item) else {
        return;
    };
    let counterpart_side = match correlation.side {
        HistoryToolSide::Call => HistoryToolSide::Output,
        HistoryToolSide::Output => HistoryToolSide::Call,
    };

    // Function outputs historically prefer a FunctionCall over LocalShellCall when malformed
    // history contains both with the same call ID. Preserve that edge-case ordering while the
    // compatibility representation still distinguishes those two call variants.
    if let ResponseItem::FunctionCallOutput {
        call_id: Some(call_id),
        ..
    } = item
    {
        if let Some(pos) = items.iter().position(|envelope| {
            matches!(&envelope.item, ResponseItem::FunctionCall { call_id: existing, .. } if existing == call_id)
        }) {
            items.remove(pos);
            return;
        }
        if let Some(pos) = items.iter().position(|envelope| {
            matches!(&envelope.item, ResponseItem::LocalShellCall { call_id: Some(existing), .. } if existing == call_id)
        }) {
            items.remove(pos);
        }
        return;
    }

    if let Some(pos) = items.iter().position(|envelope| {
        tool_correlation(&envelope.item).is_some_and(|candidate| {
            candidate.compatibility_pairing_class == correlation.compatibility_pairing_class
                && candidate.side == counterpart_side
                && candidate.call_id == correlation.call_id
        })
    }) {
        items.remove(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::LocalShellAction;
    use codex_protocol::models::LocalShellExecAction;
    use codex_protocol::models::LocalShellStatus;

    #[test]
    fn function_output_removal_prefers_function_call_over_local_shell_call() {
        let call_id = "shared-call";
        let mut items = vec![
            ResponseItemEnvelope::new(ResponseItem::LocalShellCall {
                id: None,
                call_id: Some(call_id.to_string()),
                status: LocalShellStatus::Completed,
                action: LocalShellAction::Exec(LocalShellExecAction {
                    command: vec!["echo".to_string(), "local".to_string()],
                    timeout_ms: None,
                    working_directory: None,
                    env: None,
                    user: None,
                }),
                internal_chat_message_metadata_passthrough: None,
            }),
            ResponseItemEnvelope::new(ResponseItem::FunctionCall {
                id: None,
                name: "do_it".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: call_id.to_string(),
                encrypted_function_args: None,
                internal_chat_message_metadata_passthrough: None,
            }),
        ];
        let output = ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(call_id.to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text("ok".to_string()),
            internal_chat_message_metadata_passthrough: None,
        };

        remove_corresponding_for(&mut items, &output);

        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0].item,
            ResponseItem::LocalShellCall {
                call_id: Some(remaining_call_id),
                ..
            } if remaining_call_id.as_str() == call_id
        ));
    }
}

/// Strip image content from messages and tool outputs when the model does not support images.
/// When `input_modalities` contains `InputModality::Image`, no stripping is performed.
pub(crate) fn strip_images_when_unsupported(
    input_modalities: &[InputModality],
    items: &mut [ResponseItemEnvelope],
) {
    let supports_images = input_modalities.contains(&InputModality::Image);
    if supports_images {
        return;
    }

    for envelope in items.iter_mut() {
        match &mut envelope.item {
            ResponseItem::Message { .. } => {
                let Some(mut content) = to_annotated_content(&mut envelope.item) else {
                    continue;
                };
                for content_item in &mut content {
                    if matches!(content_item.content(), ContentItem::InputImage { .. }) {
                        *content_item = UnsupportedMedia::IMAGE.render_fragment().into_parts().1;
                    }
                }
                let _ = set_annotated_content(&mut envelope.item, content);
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content_items) = output.content_items_mut() {
                    let mut normalized_content_items = Vec::with_capacity(content_items.len());
                    for content_item in content_items.iter() {
                        match content_item {
                            FunctionCallOutputContentItem::InputImage { .. } => {
                                normalized_content_items.push(
                                    FunctionCallOutputContentItem::InputText {
                                        text: UnsupportedMedia::IMAGE.render(),
                                    },
                                );
                            }
                            _ => normalized_content_items.push(content_item.clone()),
                        }
                    }
                    *content_items = normalized_content_items;
                }
            }
            ResponseItem::ImageGenerationCall { result, .. } => {
                result.clear();
            }
            _ => {}
        }
    }
}

/// Strip audio content from messages and tool outputs when the model does not support audio.
/// When `input_modalities` contains `InputModality::Audio`, no stripping is performed.
pub(crate) fn strip_audio_when_unsupported(
    input_modalities: &[InputModality],
    items: &mut [ResponseItemEnvelope],
) {
    if input_modalities.contains(&InputModality::Audio) {
        return;
    }

    for envelope in items.iter_mut() {
        match &mut envelope.item {
            ResponseItem::Message { .. } => {
                let Some(mut content) = to_annotated_content(&mut envelope.item) else {
                    continue;
                };
                for content_item in &mut content {
                    if matches!(content_item.content(), ContentItem::InputAudio { .. }) {
                        *content_item = UnsupportedMedia::AUDIO.render_fragment().into_parts().1;
                    }
                }
                let _ = set_annotated_content(&mut envelope.item, content);
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content_items) = output.content_items_mut() {
                    for content_item in content_items.iter_mut() {
                        if matches!(
                            content_item,
                            FunctionCallOutputContentItem::InputAudio { .. }
                        ) {
                            *content_item = FunctionCallOutputContentItem::InputText {
                                text: UnsupportedMedia::AUDIO.render(),
                            };
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
