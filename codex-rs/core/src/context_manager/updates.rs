use crate::context::ContextualUserFragment;
use codex_context_fragments::RenderedFragment;
use codex_history::CodexHarnessMetadata;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ContentItem;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::ResponseItem;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageGroup {
    Standalone,
    Mergeable,
}

pub(crate) fn build_rendered_message(fragments: Vec<RenderedFragment>) -> Option<ResponseItem> {
    let role = fragments.first()?.role();
    debug_assert!(fragments.iter().all(|fragment| fragment.role() == role));
    let (content, content_item_kinds): (Vec<_>, Vec<_>) = fragments
        .into_iter()
        .map(|fragment| fragment.into_parts().1.into_parts())
        .unzip();

    Some(ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: Some(InternalChatMessageMetadataPassthrough {
            content_item_kinds: Some(content_item_kinds),
            ..Default::default()
        }),
    })
}

/// Adds reinjectable-context metadata to safe text items from explicit full-context producers.
///
/// Call this only for items returned by full-context, WorldState-diff, or turn-context injection
/// producers. Role and content shape only reject unsupported items; they do not establish that an
/// arbitrary item came from a complete, rebuildable context bundle.
pub(crate) fn context_injection_envelope(item: ResponseItem) -> ResponseItemEnvelope {
    let reinjectable_context = matches!(
        &item,
        ResponseItem::Message { role, content, .. }
            if matches!(role.as_str(), "developer" | "user")
                && !content.is_empty()
                && content
                    .iter()
                    .all(|content| matches!(content, ContentItem::InputText { .. }))
    );
    if reinjectable_context {
        ResponseItemEnvelope {
            item,
            metadata: Some(CodexHarnessMetadata {
                reinjectable_context: true,
                ..Default::default()
            }),
        }
    } else {
        ResponseItemEnvelope::new(item)
    }
}

pub(crate) fn merge_contextual_fragments(
    fragments: Vec<Box<dyn ContextualUserFragment>>,
) -> Vec<ResponseItem> {
    let mut messages: Vec<(&str, MessageGroup, Vec<RenderedFragment>)> =
        Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let group = if fragment.requires_separate_message() {
            MessageGroup::Standalone
        } else {
            MessageGroup::Mergeable
        };
        let rendered = fragment.render_fragment();
        let role = rendered.role();
        match messages.last_mut() {
            Some((previous_role, previous_group, rendered_fragments))
                if *previous_role == role
                    && *previous_group == MessageGroup::Mergeable
                    && group == MessageGroup::Mergeable =>
            {
                rendered_fragments.push(rendered);
            }
            _ => messages.push((role, group, vec![rendered])),
        }
    }
    messages
        .into_iter()
        .filter_map(|(_, _, fragments)| build_rendered_message(fragments))
        .collect()
}

#[cfg(test)]
mod tests {
    use codex_history::CodexHarnessMetadata;
    use codex_history::ResponseItemEnvelope;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ContentItemKind;
    use codex_protocol::models::ImageDetail;
    use codex_protocol::models::InternalChatMessageMetadataPassthrough;
    use codex_protocol::models::ResponseItem;

    use super::context_injection_envelope;

    fn message(
        role: &str,
        content: Vec<ContentItem>,
        internal_metadata: Option<InternalChatMessageMetadataPassthrough>,
    ) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: role.to_string(),
            content,
            phase: None,
            internal_chat_message_metadata_passthrough: internal_metadata,
        }
    }

    fn text_message(role: &str) -> ResponseItem {
        message(
            role,
            vec![ContentItem::InputText {
                text: "rebuildable context".to_string(),
            }],
            None,
        )
    }

    #[test]
    fn context_injection_envelope_marks_only_developer_and_user_text() {
        for role in ["developer", "user"] {
            let item = text_message(role);
            let envelope = context_injection_envelope(item.clone());

            assert_eq!(envelope.item, item);
            assert_eq!(
                envelope.metadata,
                Some(CodexHarnessMetadata {
                    reinjectable_context: true,
                    ..Default::default()
                })
            );
        }

        let unchanged_text = text_message("developer");
        assert_eq!(
            ResponseItemEnvelope::new(unchanged_text),
            ResponseItemEnvelope {
                item: text_message("developer"),
                metadata: None,
            }
        );
    }

    #[test]
    fn context_injection_envelope_preserves_existing_item_metadata() {
        let internal_metadata = InternalChatMessageMetadataPassthrough {
            turn_id: Some("existing-turn".to_string()),
            content_item_kinds: Some(vec![ContentItemKind("unknown".to_string())]),
            ..Default::default()
        };
        let item = message(
            "developer",
            vec![ContentItem::InputText {
                text: "rebuildable context".to_string(),
            }],
            Some(internal_metadata),
        );

        assert_eq!(
            ResponseItemEnvelope::new(item.clone()),
            ResponseItemEnvelope {
                item: item.clone(),
                metadata: None,
            }
        );

        let envelope = context_injection_envelope(item.clone());

        assert_eq!(envelope.item, item);
        assert_eq!(
            envelope.metadata,
            Some(CodexHarnessMetadata {
                reinjectable_context: true,
                ..Default::default()
            })
        );
    }

    #[test]
    fn context_injection_envelope_leaves_unsupported_shapes_unmarked() {
        for role in ["assistant", "system", "extension.custom"] {
            let item = text_message(role);
            let envelope = context_injection_envelope(item.clone());
            assert_eq!(envelope.item, item);
            assert_eq!(envelope.metadata, None);
        }

        let media_message = message(
            "user",
            vec![
                ContentItem::InputText {
                    text: "caption".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,AA==".to_string(),
                    detail: Some(ImageDetail::Low),
                },
            ],
            None,
        );
        let media_envelope = context_injection_envelope(media_message.clone());
        assert_eq!(media_envelope.item, media_message);
        assert_eq!(media_envelope.metadata, None);

        let non_message = ResponseItem::Other;
        let non_message_envelope = context_injection_envelope(non_message.clone());
        assert_eq!(non_message_envelope.item, non_message);
        assert_eq!(non_message_envelope.metadata, None);
    }
}
