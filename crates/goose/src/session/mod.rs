mod chat_history_search;
mod diagnostics;
mod export_html;
mod export_markdown;
pub mod extension_data;
pub mod import_formats;
mod last_message_snippet;
mod legacy;
#[cfg(feature = "nostr")]
pub mod nostr_share;
pub mod session_manager;
mod session_naming;

pub use diagnostics::{
    config_path, generate_diagnostics, get_system_info, latest_llm_log_path, read_capped,
    read_tail, recent_cli_log_paths, DiagnosticsConfig, DiagnosticsError, DiagnosticsExtensions,
    DiagnosticsLevel, DiagnosticsLogs, DiagnosticsPrompt, DiagnosticsReport,
    DiagnosticsScheduledRecipe, DiagnosticsTextFile, SystemInfo,
};
pub use export_html::export_session_to_html;
pub use export_markdown::{
    export_session_to_markdown, message_to_markdown, user_projected_message_to_markdown,
};
pub use extension_data::{EnabledExtensionsState, ExtensionData, ExtensionState, TodoState};
pub use session_manager::{
    Session, SessionInsights, SessionManager, SessionNameUpdate, SessionType, SessionUpdateBuilder,
};
