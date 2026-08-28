use codex_api::AccessPrograms;
use codex_model_provider::ProviderAuthMetadata;
use codex_protocol::turn_input::CyberAccessProgram;

pub(crate) fn for_auth(
    auth_metadata: ProviderAuthMetadata,
    program: Option<CyberAccessProgram>,
) -> Option<AccessPrograms> {
    program
        .filter(|_| auth_metadata.is_chatgpt_auth())
        .map(AccessPrograms::from)
}
