use codex_api::SearchInput;
use codex_core::parse_turn_item;
use codex_history::HistoryItem;
use codex_history::HistoryItemProjection;
use codex_history::HistoryMessageRole;
use codex_history::ResponseItemEnvelope;
use codex_history::project_response_item;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::plaintext_agent_message_content;
use codex_tools::retain_tail_from_last_n_user_messages;
use codex_tools::truncate_assistant_output_text_to_token_budget;

const ASSISTANT_CONTEXT_TOKEN_LIMIT: usize = 1_000;
const ASSISTANT_ROLE: &str = "assistant";
#[cfg(test)]
const USER_ROLE: &str = "user";

/// Builds the conversation tail for standalone web search.
///
/// The tail keeps the previous user text message, up to 1k tokens of assistant
/// text that followed it, and the current user text message.
pub(crate) fn recent_input(items: &[ResponseItem]) -> Option<SearchInput> {
    let mut messages = Vec::new();
    for item in items {
        push_visible_message(&mut messages, item);
    }

    retain_tail_from_last_n_user_messages(&mut messages, /*user_message_count*/ 2);
    truncate_assistant_output_text_to_token_budget(&mut messages, ASSISTANT_CONTEXT_TOKEN_LIMIT);
    (!messages.is_empty()).then_some(SearchInput::Items(messages))
}

fn push_visible_message(messages: &mut Vec<ResponseItem>, item: &ResponseItem) {
    match project_response_item(ResponseItemEnvelope::new(item.clone())) {
        HistoryItemProjection::Canonical {
            item: HistoryItem::Message(message),
            compatibility,
        } => match message.role {
            HistoryMessageRole::Assistant => {
                let mut message = compatibility.item;
                message.set_id(/*new_id*/ None);
                messages.push(message);
            }
            HistoryMessageRole::User => {
                if !matches!(
                    parse_turn_item(&compatibility.item),
                    Some(TurnItem::UserMessage(_))
                ) {
                    return;
                }

                let mut message = compatibility.item;
                let ResponseItem::Message { content, .. } = &mut message else {
                    return;
                };
                content.retain(|item| matches!(item, ContentItem::InputText { .. }));
                if !content.is_empty() {
                    message.set_id(/*new_id*/ None);
                    messages.push(message);
                }
            }
            HistoryMessageRole::System | HistoryMessageRole::Developer => {}
        },
        HistoryItemProjection::Canonical { .. } => {}
        HistoryItemProjection::Fallback { compatibility, .. } => {
            if let ResponseItem::AgentMessage {
                author,
                content,
                internal_chat_message_metadata_passthrough: metadata,
                ..
            } = compatibility.item
                && let Some(text) = plaintext_agent_message_content(&content)
            {
                messages.push(ResponseItem::Message {
                    id: None,
                    role: ASSISTANT_ROLE.to_string(),
                    content: vec![ContentItem::OutputText {
                        text: format!("Agent message from {author}:\n{text}"),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: metadata,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use codex_api::SearchInput;
    use codex_protocol::ResponseItemId;
    use codex_protocol::items::HookPromptFragment;
    use codex_protocol::items::build_hook_prompt_message;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ImageDetail;
    use codex_protocol::models::InternalChatMessageMetadataPassthrough;
    use codex_protocol::models::MessagePhase;
    use codex_protocol::models::ResponseItem;
    use pretty_assertions::assert_eq;

    use super::ASSISTANT_ROLE;
    use super::USER_ROLE;
    use super::recent_input;

    fn message(role: &str, text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: role.to_string(),
            content: vec![if role == ASSISTANT_ROLE {
                ContentItem::OutputText {
                    text: text.to_string(),
                }
            } else {
                ContentItem::InputText {
                    text: text.to_string(),
                }
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn metadata(turn_id: &str) -> InternalChatMessageMetadataPassthrough {
        InternalChatMessageMetadataPassthrough {
            turn_id: Some(turn_id.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_current_user_and_previous_visible_turn() {
        let mut previous_user = message(USER_ROLE, "previous user");
        previous_user.set_id(Some(ResponseItemId::with_suffix("msg", "previous_user")));
        let mut previous_assistant = message(ASSISTANT_ROLE, "previous assistant");
        previous_assistant.set_id(Some(ResponseItemId::with_suffix(
            "msg",
            "previous_assistant",
        )));
        let items = vec![
            message("system", "system"),
            message(USER_ROLE, "old user"),
            message(ASSISTANT_ROLE, "old assistant"),
            previous_user,
            ResponseItem::FunctionCall {
                id: None,
                name: "tool".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: "call-1".to_string(),
                encrypted_function_args: None,
                internal_chat_message_metadata_passthrough: None,
            },
            previous_assistant,
            message("developer", "developer"),
            message(USER_ROLE, "current user"),
            message(ASSISTANT_ROLE, "current commentary"),
        ];

        assert_eq!(
            recent_input(&items),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                message(ASSISTANT_ROLE, "previous assistant"),
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn keeps_only_text_from_recent_user_messages() {
        let previous_user = ResponseItem::Message {
            id: None,
            role: USER_ROLE.to_string(),
            content: vec![
                ContentItem::InputText {
                    text: "previous user".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,image".to_string(),
                    detail: None,
                },
            ],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        };
        let items = vec![
            previous_user,
            message(ASSISTANT_ROLE, "previous assistant"),
            message(USER_ROLE, "current user"),
        ];

        assert_eq!(
            recent_input(&items),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                message(ASSISTANT_ROLE, "previous assistant"),
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn preserves_assistant_compatibility_payload_and_clears_id() {
        let assistant = ResponseItem::Message {
            id: Some(ResponseItemId::with_suffix("msg", "assistant")),
            role: ASSISTANT_ROLE.to_string(),
            content: vec![
                ContentItem::InputText {
                    text: "assistant input".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,image".to_string(),
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputAudio {
                    audio_url: "data:audio/wav;base64,audio".to_string(),
                },
                ContentItem::OutputText {
                    text: "assistant output".to_string(),
                },
            ],
            phase: Some(MessagePhase::Commentary),
            internal_chat_message_metadata_passthrough: Some(metadata("assistant-turn")),
        };
        let mut expected = assistant.clone();
        expected.set_id(None);

        let items = vec![
            message(USER_ROLE, "previous user"),
            assistant,
            message(USER_ROLE, "current user"),
        ];
        assert_eq!(
            recent_input(&items),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                expected,
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn filters_user_compatibility_payload_to_input_text_and_clears_id() {
        let user = ResponseItem::Message {
            id: Some(ResponseItemId::with_suffix("msg", "user")),
            role: USER_ROLE.to_string(),
            content: vec![
                ContentItem::InputText {
                    text: "first".to_string(),
                },
                ContentItem::OutputText {
                    text: "discard output".to_string(),
                },
                ContentItem::InputImage {
                    image_url: "data:image/png;base64,image".to_string(),
                    detail: Some(ImageDetail::Original),
                },
                ContentItem::InputAudio {
                    audio_url: "data:audio/wav;base64,audio".to_string(),
                },
                ContentItem::InputText {
                    text: "second".to_string(),
                },
            ],
            phase: Some(MessagePhase::FinalAnswer),
            internal_chat_message_metadata_passthrough: Some(metadata("user-turn")),
        };
        let mut expected = user.clone();
        if let ResponseItem::Message { content, .. } = &mut expected {
            content.retain(|item| matches!(item, ContentItem::InputText { .. }));
        }
        expected.set_id(None);

        assert_eq!(
            recent_input(&[user]),
            Some(SearchInput::Items(vec![expected]))
        );
    }

    #[test]
    fn keeps_plaintext_agent_message_as_legacy_fallback() {
        let agent = ResponseItem::AgentMessage {
            id: Some(ResponseItemId::with_suffix("amsg", "agent")),
            author: "worker".to_string(),
            recipient: "user".to_string(),
            content: vec![
                codex_protocol::models::AgentMessageInputContent::InputText {
                    text: "first".to_string(),
                },
                codex_protocol::models::AgentMessageInputContent::InputText {
                    text: "second".to_string(),
                },
            ],
            internal_chat_message_metadata_passthrough: Some(metadata("agent-turn")),
        };

        let expected = ResponseItem::Message {
            id: None,
            role: ASSISTANT_ROLE.to_string(),
            content: vec![ContentItem::OutputText {
                text: "Agent message from worker:\nfirst\nsecond".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(metadata("agent-turn")),
        };
        assert_eq!(
            recent_input(&[
                message(USER_ROLE, "previous user"),
                agent,
                message(USER_ROLE, "current user"),
            ]),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                expected,
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn ignores_contextual_user_messages_when_selecting_recent_turns() {
        let items = vec![
            message(USER_ROLE, "previous user"),
            message(ASSISTANT_ROLE, "previous assistant"),
            message(
                USER_ROLE,
                "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>",
            ),
            message(USER_ROLE, "current user"),
        ];

        assert_eq!(
            recent_input(&items),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                message(ASSISTANT_ROLE, "previous assistant"),
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn ignores_hook_and_unsupported_items() {
        let hook = build_hook_prompt_message(&[HookPromptFragment::from_single_hook(
            "retry this turn",
            "hook-run-1",
        )])
        .expect("hook prompt message");
        let items = vec![
            message(USER_ROLE, "previous user"),
            hook,
            ResponseItem::Other,
            ResponseItem::FunctionCall {
                id: None,
                name: "tool".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                encrypted_function_args: None,
                call_id: "call-1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            message(USER_ROLE, "current user"),
        ];

        assert_eq!(
            recent_input(&items),
            Some(SearchInput::Items(vec![
                message(USER_ROLE, "previous user"),
                message(USER_ROLE, "current user"),
            ]))
        );
    }

    #[test]
    fn preserves_assistant_truncation_after_projection() {
        let long_text = "a".repeat(4_004);
        let items = vec![
            message(USER_ROLE, "previous user"),
            message(ASSISTANT_ROLE, &long_text),
            message(ASSISTANT_ROLE, "after budget"),
            message(USER_ROLE, "current user"),
        ];

        let Some(SearchInput::Items(messages)) = recent_input(&items) else {
            panic!("expected search input items");
        };
        assert_eq!(messages.len(), 3);
        let ResponseItem::Message { content, .. } = &messages[1] else {
            panic!("expected assistant message");
        };
        let Some(ContentItem::OutputText { text }) = content.first() else {
            panic!("expected assistant output text");
        };
        assert_ne!(text, &long_text);
        assert!(!messages.iter().any(|item| {
            matches!(
                item,
                ResponseItem::Message { content, .. }
                    if content.iter().any(|item| matches!(
                        item,
                        ContentItem::OutputText { text } if text == "after budget"
                    ))
            )
        }));
    }

    #[test]
    fn returns_none_for_empty_history() {
        assert_eq!(recent_input(&[]), None);
    }
}
