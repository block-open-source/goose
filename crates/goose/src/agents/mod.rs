mod agent;
pub mod container;
pub mod execute_commands;
pub mod extension;
pub mod extension_malware_check;
pub mod extension_manager;
pub mod final_output_tool;
pub(crate) mod gen_ai_telemetry;
mod large_response_handler;
pub mod mcp_client;
pub mod moim;
pub mod platform_extensions;
pub mod platform_tools;
pub mod prompt_manager;
pub mod reply_parts;
pub mod retry;
mod schedule_tool;
pub mod state_machine;
pub mod subagent_execution_tool;
pub(crate) mod subagent_handler;
pub(crate) mod subagent_task_config;
mod tool_confirmation_coordinator;
mod tool_confirmation_router;
pub mod tool_execution;
mod tool_schema_normalize;
pub mod types;
pub mod validate_extensions;

pub use agent::{Agent, AgentConfig, ExtensionLoadResult, GoosePlatform, MCP_PROTOCOL_VERSION};
pub use container::Container;
pub use execute_commands::{context_management_unsupported_message, COMPACT_TRIGGERS};
pub use extension::{ExtensionConfig, ExtensionError};
pub use extension_manager::ExtensionManager;
pub use goose_agent::events::AgentEvent;
pub(crate) use large_response_handler::max_tool_response_size;
pub use prompt_manager::PromptManager;
pub use schedule_tool::ScheduleTool;
pub use subagent_handler::SUBAGENT_TOOL_REQUEST_TYPE;
pub use subagent_task_config::TaskConfig;
pub use tool_execution::ToolCallContext;
pub use types::{RetryConfig, SessionConfig, SuccessCheck};

pub(crate) fn latest_provider_session_id<'a>(
    messages: &'a [crate::conversation::message::Message],
    provider: &str,
) -> Option<&'a str> {
    let inference = messages
        .iter()
        .rev()
        .find_map(|message| message.metadata.inference.as_ref())?;
    (inference.provider == provider)
        .then_some(inference.provider_session_id.as_deref())
        .flatten()
}
