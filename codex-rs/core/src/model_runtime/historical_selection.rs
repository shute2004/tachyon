use super::codex_adapter::CodexHistoricalModelSelection;

/// Opaque prior-turn model identity and adapter-private selection metadata.
///
/// The model identity is usable by generic harness behavior. Provider-specific selection details
/// remain available only to the Codex migration bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoricalModelSelection {
    model_id: String,
    codex: CodexHistoricalModelSelection,
}

impl HistoricalModelSelection {
    pub(crate) fn model_id(&self) -> &str {
        &self.model_id
    }

    pub(super) fn codex_adapter_selection(&self) -> &CodexHistoricalModelSelection {
        &self.codex
    }

    pub(super) fn from_codex_adapter(
        model_id: String,
        codex: CodexHistoricalModelSelection,
    ) -> Self {
        Self { model_id, codex }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::turn_input::CyberAccessProgram;

    #[test]
    fn equality_includes_codex_program_and_its_absence() {
        let selection = |program| {
            HistoricalModelSelection::from_codex_adapter(
                "gpt-5.4".to_string(),
                CodexHistoricalModelSelection::new(program),
            )
        };

        let blue = selection(Some(CyberAccessProgram::DaybreakBlue));
        let red = selection(Some(CyberAccessProgram::DaybreakRed));
        let absent = selection(None);

        assert_eq!(blue.model_id(), red.model_id());
        assert_ne!(blue, red);
        assert_ne!(blue, absent);
        assert_ne!(red, absent);
        assert_eq!(absent, selection(None));
    }

    #[tokio::test]
    async fn codex_turn_context_bridge_preserves_program_and_absence() {
        let (_, mut turn_context) = crate::session::tests::make_session_and_context().await;
        for program in [
            None,
            Some(CyberAccessProgram::DaybreakBlue),
            Some(CyberAccessProgram::DaybreakRed),
        ] {
            turn_context.cyber_access_program = program;
            let actual = crate::model_runtime::historical_model_selection_from_codex_turn_context(
                &turn_context,
            );
            let expected = HistoricalModelSelection::from_codex_adapter(
                turn_context.model_info().slug.clone(),
                CodexHistoricalModelSelection::new(program),
            );

            assert_eq!(actual, expected);
        }
    }

    #[tokio::test]
    async fn codex_turn_context_item_bridge_preserves_program_and_absence() {
        let (_, turn_context) = crate::session::tests::make_session_and_context().await;
        let mut turn_context_item = turn_context.to_turn_context_item();
        for program in [
            None,
            Some(CyberAccessProgram::DaybreakBlue),
            Some(CyberAccessProgram::DaybreakRed),
        ] {
            turn_context_item.cyber_access_program = program;
            let actual =
                crate::model_runtime::historical_model_selection_from_codex_turn_context_item(
                    &turn_context_item,
                );
            let expected = HistoricalModelSelection::from_codex_adapter(
                turn_context_item.model.clone(),
                CodexHistoricalModelSelection::new(program),
            );

            assert_eq!(actual, expected);
        }
    }

    #[tokio::test]
    async fn restores_historical_model_and_program_without_mutating_current_context() {
        let (session, mut current) = crate::session::tests::make_session_and_context().await;
        let current_model = current.model_info().slug.clone();
        let historical_model = if current_model == "gpt-5.4" {
            "gpt-5.2"
        } else {
            "gpt-5.4"
        };
        current.cyber_access_program = Some(CyberAccessProgram::DaybreakRed);

        let historical_with_program = HistoricalModelSelection::from_codex_adapter(
            historical_model.to_string(),
            CodexHistoricalModelSelection::new(Some(CyberAccessProgram::DaybreakBlue)),
        );
        let restored_with_program =
            crate::model_runtime::codex_turn_context_for_historical_selection(
                &current,
                &historical_with_program,
                &session.services.models_manager,
            )
            .await;
        assert_eq!(restored_with_program.model_info().slug, historical_model);
        assert_eq!(
            restored_with_program.cyber_access_program,
            Some(CyberAccessProgram::DaybreakBlue)
        );

        let historical_without_program = HistoricalModelSelection::from_codex_adapter(
            historical_model.to_string(),
            CodexHistoricalModelSelection::new(None),
        );
        let restored_without_program =
            crate::model_runtime::codex_turn_context_for_historical_selection(
                &current,
                &historical_without_program,
                &session.services.models_manager,
            )
            .await;
        assert_eq!(restored_without_program.model_info().slug, historical_model);
        assert_eq!(restored_without_program.cyber_access_program, None);

        assert_eq!(current.model_info().slug, current_model);
        assert_eq!(
            current.cyber_access_program,
            Some(CyberAccessProgram::DaybreakRed)
        );
    }
}
