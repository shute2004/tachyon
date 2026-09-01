mod capabilities;
mod contributors;
mod registry;
mod state;
mod user_instructions;

/// Immutable MCP tool metadata exposed to extension lifecycle contributors.
///
/// This is a snapshot of the exact MCP call selected by the host. It keeps the
/// raw server/tool identity needed for correlation alongside the model-visible
/// callable name, while leaving the executable MCP client, raw tool definition,
/// and provider-specific file-input metadata inside the host runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpToolInfo {
    /// Raw MCP server name used to route the call.
    pub server_name: String,
    /// Raw MCP tool name used by the protocol call.
    pub tool_name: String,
    /// Model-visible callable name.
    pub callable_name: String,
    /// Model-visible callable namespace.
    pub callable_namespace: String,
    /// Model-visible namespace description, when provided by the server or connector.
    pub namespace_description: Option<String>,
    /// Whether the server advertises support for parallel tool calls.
    pub supports_parallel_tool_calls: bool,
    /// MCP server origin used for telemetry and diagnostics, when known.
    pub server_origin: Option<String>,
    /// Connector identity, when the tool came from a connector-backed MCP server.
    pub connector_id: Option<String>,
    /// Connector display name, when the tool came from a connector-backed MCP server.
    pub connector_name: Option<String>,
    /// Display names of plugins that expose or include this tool.
    pub plugin_display_names: Vec<String>,
}

impl McpToolInfo {
    /// Returns the model-visible callable name with its namespace.
    pub fn canonical_tool_name(&self) -> ToolName {
        ToolName::namespaced(self.callable_namespace.clone(), self.callable_name.clone())
    }
}

pub use capabilities::AgentSpawnFuture;
pub use capabilities::AgentSpawner;
pub use capabilities::ConversationHistorySnapshot;
pub use capabilities::ExtensionEventSink;
pub use capabilities::ExtensionMetrics;
pub use capabilities::ExtensionWarning;
pub use capabilities::InternalSessionSpawnFuture;
pub use capabilities::InternalSessionSpawner;
pub use capabilities::NoopExtensionEventSink;
pub use capabilities::NoopResponseItemInjector;
pub use capabilities::ResponseItemInjectionFuture;
pub use capabilities::ResponseItemInjector;
pub use codex_context_fragments::ContextualUserFragment;
pub use codex_protocol::models::ContentItemKind;
pub use codex_protocol::models::ResponseItem;
pub use codex_protocol::security_risk::SecurityRiskScore;
pub use codex_tools::ConversationHistory;
pub use codex_tools::ExtensionTurnItem;
pub use codex_tools::FunctionCallError;
pub use codex_tools::JsonToolOutput;
pub use codex_tools::NoopTurnItemEmitter;
pub use codex_tools::ResponsesApiTool;
pub use codex_tools::ToolCall;
pub use codex_tools::ToolCallSource;
pub use codex_tools::ToolEnvironment;
pub use codex_tools::ToolExecutor;
pub use codex_tools::ToolExecutorFuture;
pub use codex_tools::ToolName;
pub use codex_tools::ToolOutput;
pub use codex_tools::ToolPayload;
pub use codex_tools::ToolSpec;
pub use codex_tools::TurnItemEmissionFuture;
pub use codex_tools::TurnItemEmitter;
pub use codex_tools::parse_tool_input_schema;
pub use codex_tools::parse_tool_input_schema_without_compaction;
pub use contributors::ApprovalAssessment;
pub use contributors::ApprovalReviewContributor;
pub use contributors::ApprovalReviewError;
pub use contributors::ApprovalReviewInput;
pub use contributors::ConfigContributor;
pub use contributors::ContextContributor;
pub use contributors::ExtensionFuture;
pub use contributors::McpServerContribution;
pub use contributors::McpServerContributionContext;
pub use contributors::McpServerContributor;
pub use contributors::McpToolContext;
pub use contributors::McpToolSource;
pub use contributors::PreviousWorldStateSection;
pub use contributors::PromptFragment;
pub use contributors::PromptSlot;
pub use contributors::RenderedWorldStateFragment;
pub use contributors::SelectedPluginIdentity;
pub use contributors::SelectedPluginSnapshot;
pub use contributors::SkillInvocationContributor;
pub use contributors::SkillInvocationInput;
pub use contributors::SkillInvocationKind;
pub use contributors::ThreadIdleCause;
pub use contributors::ThreadIdleInput;
pub use contributors::ThreadLifecycleContributor;
pub use contributors::ThreadOriginator;
pub use contributors::ThreadReadyInput;
pub use contributors::ThreadResumeInput;
pub use contributors::ThreadStartInput;
pub use contributors::ThreadStopInput;
pub use contributors::TokenUsageContributor;
pub use contributors::ToolCallOutcome;
pub use contributors::ToolContributor;
pub use contributors::ToolFinishInput;
pub use contributors::ToolLifecycleContributor;
pub use contributors::ToolLifecycleFuture;
pub use contributors::ToolStartInput;
pub use contributors::TurnAbortInput;
pub use contributors::TurnContextContributionInput;
pub use contributors::TurnErrorInput;
pub use contributors::TurnInputContext;
pub use contributors::TurnInputContributor;
pub use contributors::TurnInputEnvironment;
pub use contributors::TurnItemContributor;
pub use contributors::TurnLifecycleContributor;
pub use contributors::TurnStartInput;
pub use contributors::TurnStopInput;
pub use contributors::WorldStateContributionInput;
pub use contributors::WorldStateSectionContribution;
pub use registry::ExtensionRegistry;
pub use registry::ExtensionRegistryBuilder;
pub use registry::empty_extension_registry;
pub use state::ExtensionData;
pub use state::ExtensionDataInit;
pub use user_instructions::Instructions;
pub use user_instructions::LoadUserInstructionsFuture;
pub use user_instructions::LoadedUserInstructions;
pub use user_instructions::UserInstructionsProvider;
