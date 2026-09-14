#![recursion_limit = "256"]

#[cfg(all(feature = "rustls-tls", feature = "native-tls"))]
compile_error!("Features `rustls-tls` and `native-tls` are mutually exclusive");

pub mod acp;
pub use goose_sdk_types::{custom_notifications, custom_requests};
pub mod action_required_manager;
pub mod agents;
pub mod builtin_extension;
pub mod checks;
pub mod config;
pub mod context_limit;
pub mod context_mgmt;
pub mod conversation {
    pub use goose_providers::conversation::*;
}
pub mod dictation;
pub mod doctor;
pub mod download_manager;
pub mod elicitation;
pub mod execution;
pub mod gateway;
pub mod goose_apps;
pub mod hints;
pub mod hooks;
pub mod instance_id;
pub mod logging;
pub mod mcp_utils;
pub mod model_config;
pub mod oauth;
#[cfg(feature = "otel")]
pub mod otel;
pub mod permission;
pub mod plugins;
#[cfg(feature = "telemetry")]
pub mod posthog;
pub mod prompt_template;
pub mod providers;
pub mod recipe;
pub mod recipe_deeplink;
pub mod scheduler;
pub mod scheduler_trait;
pub mod security;
pub mod session;
pub mod session_context;
pub mod skills;
pub mod slash_commands;
pub mod source_roots;
pub mod sources;
pub mod subprocess;
pub mod token_counter;
mod tool_call_labels;
pub mod tool_inspection;
pub mod tool_monitor;
pub mod tracing;
pub mod utils;
