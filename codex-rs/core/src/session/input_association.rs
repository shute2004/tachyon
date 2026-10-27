use super::session::Session;
use super::turn_context::TurnContext;
use codex_history::CodexHarnessMetadata;
use codex_history::InputAssociation;
use codex_history::InputSource;
use codex_history::ResponseItemEnvelope;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::user_input::UserInput;
use codex_thread_store::PersistContext;
use tracing::warn;

/// Reserves an identity for an already accepted reply-bearing input.
///
/// Reservation failure is intentionally best-effort here: accepted work must keep its existing
/// behavior even when the store cannot reserve an identity. This association is descriptive data,
/// not proof of admission, persistence, source, or transcript completeness.
pub(super) async fn reserve_input_association(
    session: &Session,
    source: InputSource,
) -> Option<InputAssociation> {
    let reservation = match session
        .services
        .thread_store
        .reserve_input_identity(session.thread_id)
        .await
    {
        Ok(reservation) => reservation,
        Err(error) => {
            warn!(
                thread_id = %session.thread_id,
                %error,
                "could not reserve input identity; accepted input will be recorded without an association"
            );
            return None;
        }
    };

    if reservation.identity.thread_id != session.thread_id {
        warn!(
            requested_thread_id = %session.thread_id,
            reserved_thread_id = %reservation.identity.thread_id,
            "thread store returned an input identity for another thread; accepted input will be recorded without an association"
        );
        return None;
    }

    Some(InputAssociation {
        identity: reservation.identity,
        source,
    })
}

impl Session {
    pub(crate) async fn record_user_prompt_and_emit_turn_item(
        &self,
        turn_context: &TurnContext,
        input: &[UserInput],
        client_id: Option<String>,
        input_association: Option<InputAssociation>,
        persist_context: PersistContext,
    ) {
        // Persist the user message to history, but emit the turn item from `UserInput` so
        // UI-only `text_elements` are preserved. `ResponseItem::Message` does not carry
        // those spans, and `record_response_item_and_emit_turn_item` would drop them.
        let response_item = self.response_item_from_user_input(input.to_vec());
        if let Some(input_association) = input_association {
            self.record_annotated_conversation_items(
                turn_context,
                vec![ResponseItemEnvelope {
                    item: response_item,
                    metadata: Some(CodexHarnessMetadata {
                        input_association: Some(input_association),
                        ..Default::default()
                    }),
                }],
            )
            .await;
        } else {
            self.record_conversation_items(turn_context, std::slice::from_ref(&response_item))
                .await;
        }

        let mut user_message_item = UserMessageItem::new(input);
        user_message_item.client_id = client_id;
        let turn_item = TurnItem::UserMessage(user_message_item);
        self.emit_turn_item_started(turn_context, &turn_item).await;
        self.emit_turn_item_completed(turn_context, turn_item).await;
        self.ensure_rollout_materialized(persist_context).await;
    }
}

#[cfg(test)]
#[path = "input_association_tests.rs"]
mod tests;
