use crate::acp::custom_notifications::*;
use crate::acp::custom_requests::*;
use crate::acp::fs::AcpTools;
pub(super) use crate::acp::response_builder::{
    agent_thinking_effort_support, build_config_options, build_mode_state, build_model_state,
    build_provider_options, build_session_info, build_session_setup_config,
    send_session_setup_notifications, session_meta, session_provider_selection,
    session_response_meta, should_refresh_inventory_for_session_init,
};
use crate::acp::tool_call_notifier::ToolCallNotifier;
use crate::acp::{PermissionDecision, ACP_CURRENT_MODEL};
use crate::agents::extension::{Envs, PLATFORM_EXTENSIONS};
use crate::agents::mcp_client::{GooseMcpHostInfo, McpClientTrait};
use crate::agents::platform_extensions::developer::DeveloperClient;
use crate::agents::state_machine::{
    has_unapplied_tool_confirmation_response, pending_tool_confirmations,
};
use crate::agents::{
    Agent, AgentConfig, ExtensionConfig, ExtensionLoadResult, GoosePlatform, SessionConfig,
};
use crate::config::base::CONFIG_YAML_NAME;
use crate::config::extensions::{configured_enabled_state, get_enabled_extensions_with_config};
use crate::config::paths::Paths;
use crate::config::permission::PermissionManager;
use crate::config::{Config, GooseMode};
use crate::conversation::message::{
    ActionRequiredData, Message, MessageContent, SystemNotificationContent, SystemNotificationType,
    ToolConfirmationRequest, ToolRequest, ToolResponse,
};
use crate::conversation::Conversation;
use crate::execution::manager::{AgentManager, AgentManagerGetResult, RuntimeContext};
use crate::permission::permission_confirmation::PrincipalType;
use crate::permission::{Permission, PermissionConfirmation};
use crate::providers::base::Provider;
use crate::providers::inventory::{
    ProviderInventoryEntry, ProviderInventoryService, RefreshJobPlan, RefreshPlan,
    RefreshSkipReason,
};
use crate::scheduler_trait::SchedulerTrait;
use crate::session::session_manager::SessionUsageTotals;
use crate::session::{
    EnabledExtensionsState, ExtensionData, ExtensionState, Session, SessionManager, SessionType,
};
use crate::source_roots::SourceRoot;
use crate::utils::sanitize_unicode_tags;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, Annotations, AuthMethod, AuthMethodAgent, AuthenticateRequest,
    AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ConfigOptionUpdate, ContentBlock, Cost, CurrentModeUpdate, DeleteSessionRequest,
    DeleteSessionResponse, EmbeddedResourceResource, FileSystemCapabilities, ForkSessionRequest,
    ForkSessionResponse, ImageContent, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    McpCapabilities, McpServer, Meta, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptCapabilities, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, ResourceLink, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigOption, SessionDeleteCapabilities, SessionId,
    SessionInfoUpdate, SessionListCapabilities, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent, ToolCallId, ToolCallUpdate, Usage,
    UsageUpdate,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    Agent as SacpAgent, ByteStreams, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled,
    Responder,
};
use anyhow::Result;
use fs_err as fs;
use futures::future::{BoxFuture, FutureExt};
use futures::stream::{self, BoxStream, StreamExt};
use goose_providers::errors::ProviderError;
use rmcp::model::{
    Annotations as RmcpAnnotations, ImageContent as RmcpImageContent, Role,
    TextContent as RmcpTextContent,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, OnceCell};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use url::Url;
use uuid::Uuid;

use self::message_meta::{
    content_chunk_for_message, message_meta_without_steer, populate_output_token_limit_content,
};
use self::tool_calls::chain::{breaks_consecutive_tool_calls, ReadyToolChain, ToolChainTracker};
use self::tool_calls::conversion::{
    build_initial_tool_call_with_message_meta, build_permission_tool_call_update,
    tool_call_update_fields_from_response, trusted_update_meta,
};
use self::tool_calls::enrichment::{spawn_chain_summary_enrichment, spawn_tool_title_enrichment};

mod agent_requests;
pub use agent_requests::agent_request_schemas;
mod agent_mentions;
mod apps;
mod config;
mod custom_dispatch;
mod diagnostics;
mod dictation;
mod dispatch;
mod elicitation;
mod extensions;
mod fork_session;
mod list_sessions;
mod load_session;
mod local_inference;
mod manage_sessions;
mod message_meta;
mod new_session;
mod onboarding;
mod prompts;
mod providers;
mod recipe;
mod resources;
mod schedule;
mod slash_commands;
mod sources;
mod tool_calls;
mod tool_notifications;
mod tools;

pub type AcpProviderFactory = Arc<
    dyn Fn(
            String,
            Vec<ExtensionConfig>,
            Option<PathBuf>,
            bool,
        ) -> BoxFuture<'static, Result<Arc<dyn Provider>>>
        + Send
        + Sync,
>;

const ACP_VISIBLE_SESSION_TYPES: [SessionType; 3] =
    [SessionType::User, SessionType::Scheduled, SessionType::Acp];

fn is_acp_visible_session_type(session_type: &SessionType) -> bool {
    ACP_VISIBLE_SESSION_TYPES.contains(session_type)
}

/// Convenience conversions from any `Display` error into an `agent_client_protocol::Error`.
///
/// Replaces the repetitive `.internal_err()`
/// pattern. Use `.internal_err()?` for server-side failures and `.invalid_params_err()?`
/// for bad client input. For custom messages use `.internal_err_ctx("context")?`.
#[allow(dead_code)]
trait ResultExt<T> {
    fn internal_err(self) -> Result<T, agent_client_protocol::Error>;
    fn invalid_params_err(self) -> Result<T, agent_client_protocol::Error>;
    fn internal_err_ctx(self, context: &str) -> Result<T, agent_client_protocol::Error>;
    fn invalid_params_err_ctx(self, context: &str) -> Result<T, agent_client_protocol::Error>;
}

impl<T, E: std::fmt::Display> ResultExt<T> for Result<T, E> {
    fn internal_err(self) -> Result<T, agent_client_protocol::Error> {
        self.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))
    }
    fn invalid_params_err(self) -> Result<T, agent_client_protocol::Error> {
        self.map_err(|e| agent_client_protocol::Error::invalid_params().data(e.to_string()))
    }
    fn internal_err_ctx(self, context: &str) -> Result<T, agent_client_protocol::Error> {
        self.map_err(|e| {
            agent_client_protocol::Error::internal_error().data(format!("{context}: {e}"))
        })
    }
    fn invalid_params_err_ctx(self, context: &str) -> Result<T, agent_client_protocol::Error> {
        self.map_err(|e| {
            agent_client_protocol::Error::invalid_params().data(format!("{context}: {e}"))
        })
    }
}

fn agent_creation_error(error: anyhow::Error, context: &str) -> agent_client_protocol::Error {
    if crate::acp::is_auth_required(&error) {
        agent_client_protocol::Error::auth_required()
    } else {
        agent_client_protocol::Error::internal_error().data(format!("{context}: {error}"))
    }
}

/// Only a value the client could usefully change is `invalid_params`; everything
/// else (a dead agent subprocess, a failed persist, a failed provider respawn) is
/// an operational failure the client cannot fix by picking differently.
fn thinking_effort_error(error: anyhow::Error) -> agent_client_protocol::Error {
    let base = match error.downcast_ref::<ProviderError>() {
        Some(ProviderError::InvalidValue(_)) => agent_client_protocol::Error::invalid_params(),
        _ => agent_client_protocol::Error::internal_error(),
    };
    // `{error:#}` rather than `{error}`: context layering hides the cause chain,
    // including the variant this mapping branched on.
    base.data(format!("Failed to update thinking effort: {error:#}"))
}

async fn resume_saved_provider_session(
    provider: &Arc<dyn Provider>,
    conversation: Option<&Conversation>,
) {
    let Some(conversation) = conversation else {
        return;
    };
    let provider_name = provider.get_name();
    let Some(session_id) =
        crate::agents::latest_provider_session_id(conversation.messages(), provider_name)
    else {
        return;
    };
    if let Err(error) = provider.resume(session_id).await {
        warn!(
            provider = provider_name,
            %error,
            "Could not resume provider session during ACP session setup"
        );
    }
}

pub(super) const DEFAULT_PROVIDER_ID: &str = "goose";
pub(super) const DEFAULT_PROVIDER_LABEL: &str = "Goose (Default)";
const PROVIDER_CONFIG_STATUS_CHECK_CONCURRENCY: usize = 16;

/// In-memory state for an active ACP session.
///
/// ## Terminology (temporary, until all clients migrate to ACP)
///
/// The ACP protocol uses "session" to mean the conversation as the human sees it —
/// a durable, append-only exchange of messages. Internally, goose also has a concept
/// called "Session" (the `sessions` DB table) which represents the agent's working
/// state: the message list the LLM sees, compaction state, provider binding, etc.
///
/// The ACP session ID maps directly to a `sessions` row. The `sessions` HashMap
/// below is keyed by session ID.
struct GooseAcpSession {
    agent: Arc<Agent>,
}

pub struct ActivePromptRun {
    run_id: String,
    cancel_token: CancellationToken,
    /// The agent actually running this prompt. Roaming gives each connection
    /// its own agent, so a steer arriving on a second connection must be
    /// routed here rather than to the caller's connection-local agent.
    agent: Arc<Agent>,
}

struct AgentStreamOutcome {
    was_cancelled: bool,
    output_token_limit_reached: bool,
}

/// Per-session active-run registry, shared by every `GooseAcpAgent` created
/// from one `AcpServer`. Roaming spawns a fresh agent per connection, so two
/// paired clients loading the same session get distinct agents; sharing this
/// map across them is what makes the "session already has an active run" guard
/// fire between connections instead of letting two loops interleave writes on
/// one session.
pub type ActiveRunRegistry = Arc<Mutex<HashMap<String, ActivePromptRun>>>;

/// Releases a registry entry if the task consuming an agent stream is dropped
/// without reaching its explicit `clear_active_run` — e.g. a roaming
/// connection is revoked or lost mid-turn. Without this, the shared registry
/// retains the run forever and every later connection gets "session already
/// has active run".
///
/// The explicit clear still runs on normal paths; this drop is then a no-op
/// because the entry (matched by run id) is already gone.
struct ActiveRunDropGuard {
    registry: ActiveRunRegistry,
    session_id: String,
    run_id: String,
    cancel_token: CancellationToken,
}

impl Drop for ActiveRunDropGuard {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        let registry = self.registry.clone();
        let session_id = std::mem::take(&mut self.session_id);
        let run_id = std::mem::take(&mut self.run_id);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let agent = {
                    let mut runs = registry.lock().await;
                    match runs.get(&session_id) {
                        Some(run) if run.run_id == run_id => {
                            runs.remove(&session_id).map(|run| run.agent)
                        }
                        _ => None,
                    }
                };
                if let Some(agent) = agent {
                    agent.discard_pending_steers(&session_id).await;
                }
            });
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AcpBuiltinSelection {
    pub defaults: Vec<String>,
    pub explicit: Vec<String>,
}

impl AcpBuiltinSelection {
    pub fn from_requested(builtins: Vec<String>) -> Self {
        if builtins.is_empty() {
            Self {
                defaults: vec!["developer".to_string()],
                explicit: Vec::new(),
            }
        } else {
            Self {
                defaults: Vec::new(),
                explicit: builtins,
            }
        }
    }
}

pub struct GooseAcpAgentOptions {
    pub provider_factory: AcpProviderFactory,
    pub builtin_selection: AcpBuiltinSelection,
    pub data_dir: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    pub disable_session_naming: bool,
    pub goose_platform: GoosePlatform,
    pub additional_source_roots: Vec<SourceRoot>,
    pub scheduler: Option<Arc<dyn SchedulerTrait>>,
    /// When set, new sessions use this host-controlled working directory instead
    /// of the `cwd` the connecting client sends (see `AcpServerFactoryConfig`).
    pub session_cwd: Option<std::path::PathBuf>,
    /// Active-run registry shared across all agents from one `AcpServer`, so the
    /// active-run guard holds across roaming connections that each get a fresh
    /// agent for the same session.
    pub active_prompt_runs: ActiveRunRegistry,
}

pub struct GooseAcpAgent {
    sessions: Arc<Mutex<HashMap<String, GooseAcpSession>>>,
    active_prompt_runs: Arc<Mutex<HashMap<String, ActivePromptRun>>>,
    closed_session_ids: Arc<Mutex<HashSet<String>>>,
    agent_manager: Arc<AgentManager>,
    provider_factory: AcpProviderFactory,
    builtin_selection: AcpBuiltinSelection,
    client_fs_capabilities: OnceCell<FileSystemCapabilities>,
    client_terminal: OnceCell<bool>,
    client_mcp_host_info: OnceCell<GooseMcpHostInfo>,
    client_supports_acp_elicitation: OnceCell<bool>,
    client_supports_goose_custom_notifications: OnceCell<bool>,
    client_supports_recipe_param_requests: OnceCell<bool>,
    client_requests_tool_call_label_enrichment: OnceCell<bool>,
    use_login_shell_path: OnceCell<bool>,
    client_cx: OnceCell<ConnectionTo<Client>>,
    thinking_effort_update_tx: mpsc::UnboundedSender<String>,
    thinking_effort_update_rx: Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    config_dir: std::path::PathBuf,
    session_manager: Arc<SessionManager>,
    permission_manager: Arc<PermissionManager>,
    disable_session_naming: bool,
    provider_inventory: ProviderInventoryService,
    additional_source_roots: Vec<SourceRoot>,
    session_cwd: Option<PathBuf>,
    recipe_path_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

fn meta_string(
    meta: Option<&Meta>,
    key: &str,
) -> Result<Option<String>, agent_client_protocol::Error> {
    let Some(value) = meta.and_then(|m| m.get(key)) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(
            agent_client_protocol::Error::invalid_params().data(format!("{key} must be a string"))
        );
    };
    Ok(Some(value.to_string()))
}

fn agent_capabilities_meta() -> Option<Meta> {
    let mut goose = serde_json::Map::new();
    goose.insert("recipeParameterScopes".to_string(), serde_json::json!({}));
    if cfg!(feature = "local-inference") {
        goose.insert("localInference".to_string(), serde_json::json!({}));
    }

    let mut meta = serde_json::Map::new();
    meta.insert("goose".to_string(), serde_json::Value::Object(goose));
    Some(meta)
}

fn spawn_session_name_update_notifier(
    cx: ConnectionTo<Client>,
) -> tokio::sync::mpsc::UnboundedSender<crate::session::SessionNameUpdate> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::session::SessionNameUpdate>();
    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "messageCount".to_string(),
                serde_json::Value::Number(update.message_count.into()),
            );
            meta.insert(
                "userSetName".to_string(),
                serde_json::Value::Bool(update.user_set_name),
            );
            let notification = SessionNotification::new(
                SessionId::new(update.session_id.clone()),
                SessionUpdate::SessionInfoUpdate(
                    SessionInfoUpdate::new()
                        .title(update.name)
                        .updated_at(update.updated_at.to_rfc3339())
                        .meta(meta),
                ),
            );
            if let Err(error) = cx.send_notification(notification) {
                warn!(
                    session_id = %update.session_id,
                    error = %error,
                    "Failed to send generated session name update"
                );
            }
        }
    });
    tx
}

fn extract_timeout_from_meta(meta: &Option<Meta>) -> Option<u64> {
    meta.as_ref()
        .and_then(|m| m.get("timeout"))
        .and_then(|v| v.as_u64())
}

#[derive(Debug, Default, Deserialize)]
struct ClientCapabilitiesMeta {
    #[serde(default)]
    goose: Option<GooseClientCapabilities>,
}

#[derive(Debug, Default, Deserialize)]
struct GooseClientCapabilities {
    #[serde(rename = "mcpHostCapabilities", default)]
    mcp_host_capabilities: Option<GooseMcpHostCapabilities>,
    #[serde(rename = "customNotifications", default)]
    custom_notifications: Option<bool>,
    #[serde(rename = "recipeParameterRequests", default)]
    recipe_parameter_requests: Option<bool>,
    #[serde(rename = "toolCallLabelEnrichment", default)]
    tool_call_label_enrichment: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct GooseMcpHostCapabilities {
    #[serde(default)]
    extensions: Option<rmcp::model::ExtensionCapabilities>,
}

fn extract_client_capabilities_meta(args: &InitializeRequest) -> Option<ClientCapabilitiesMeta> {
    args.client_capabilities
        .meta
        .as_ref()
        .and_then(|meta| serde_json::from_value(serde_json::Value::Object(meta.clone())).ok())
}

fn extract_client_mcp_host_info(
    args: &InitializeRequest,
    goose_client_capabilities: Option<&GooseClientCapabilities>,
) -> GooseMcpHostInfo {
    let host_capabilities =
        goose_client_capabilities.and_then(|goose| goose.mcp_host_capabilities.as_ref());
    let explicit_extensions = host_capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.extensions.as_ref())
        .is_some();
    let extensions = host_capabilities
        .and_then(|capabilities| capabilities.extensions.clone())
        .unwrap_or_default();

    GooseMcpHostInfo {
        explicit_extensions,
        extensions,
        client_name: args.client_info.as_ref().map(|info| info.name.clone()),
        client_version: args.client_info.as_ref().map(|info| info.version.clone()),
    }
}

fn extract_use_login_shell_path(args: &InitializeRequest) -> bool {
    args.meta
        .as_ref()
        .and_then(|meta| meta.get("goose/useLoginShellPath"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn mcp_server_to_extension_config(mcp_server: McpServer) -> Result<ExtensionConfig, String> {
    match mcp_server {
        McpServer::Stdio(stdio) => {
            let timeout = extract_timeout_from_meta(&stdio.meta);
            Ok(ExtensionConfig::Stdio {
                name: stdio.name,
                description: String::new(),
                cmd: stdio.command.to_string_lossy().to_string(),
                args: stdio.args,
                envs: Envs::new(stdio.env.into_iter().map(|e| (e.name, e.value)).collect()),
                env_keys: vec![],
                timeout,
                cwd: None,
                bundled: Some(false),
                available_tools: vec![],
            })
        }
        McpServer::Http(http) => {
            let timeout = extract_timeout_from_meta(&http.meta);
            Ok(ExtensionConfig::StreamableHttp {
                name: http.name,
                description: String::new(),
                uri: http.url,
                envs: Envs::default(),
                env_keys: vec![],
                headers: http
                    .headers
                    .into_iter()
                    .map(|h| (h.name, h.value))
                    .collect(),
                timeout,
                socket: None,
                client_id: None,
                client_secret_key: None,
                scopes: vec![],
                bundled: Some(false),
                available_tools: vec![],
            })
        }
        McpServer::Sse(_) => Err("SSE is unsupported, migrate to streamable_http".to_string()),
        _ => Err("Unknown MCP server type".to_string()),
    }
}

fn add_mcp_servers(
    extensions: &mut Vec<ExtensionConfig>,
    mcp_servers: Vec<McpServer>,
) -> Result<(), agent_client_protocol::Error> {
    for mcp_server in mcp_servers {
        let extension = mcp_server_to_extension_config(mcp_server)
            .map_err(|message| agent_client_protocol::Error::invalid_params().data(message))?;
        push_or_replace_extension(extensions, extension);
    }
    Ok(())
}

fn enabled_extensions_data(
    session: &Session,
    extensions: Vec<ExtensionConfig>,
) -> Result<ExtensionData, agent_client_protocol::Error> {
    let mut extension_data = session.extension_data.clone();
    EnabledExtensionsState::new(extensions)
        .to_extension_data(&mut extension_data)
        .internal_err_ctx("Failed to initialize session extensions")?;
    Ok(extension_data)
}

fn selected_builtin_extensions(
    config: &Config,
    builtin_selection: &AcpBuiltinSelection,
) -> Vec<ExtensionConfig> {
    let mut extensions = Vec::new();

    for builtin in &builtin_selection.defaults {
        if configured_enabled_state(config, builtin) != Some(false) {
            push_or_replace_extension(&mut extensions, builtin_to_extension_config(builtin));
        }
    }

    for builtin in &builtin_selection.explicit {
        push_or_replace_extension(&mut extensions, builtin_to_extension_config(builtin));
    }

    extensions
}

fn initial_session_extensions(
    config: &Config,
    builtin_selection: &AcpBuiltinSelection,
    project_root: &Path,
    mcp_servers: Vec<McpServer>,
    goose_extensions: Option<Vec<GooseExtension>>,
    recipe_extensions: Option<&[ExtensionConfig]>,
) -> Result<Vec<ExtensionConfig>, agent_client_protocol::Error> {
    let mut extensions = selected_builtin_extensions(config, builtin_selection);

    if let Some(recipe_extensions) = recipe_extensions {
        for extension in recipe_extensions {
            push_or_replace_extension(&mut extensions, extension.clone());
        }
    } else if let Some(goose_extensions) = goose_extensions {
        for extension in extensions::goose_extensions_to_configs(goose_extensions)? {
            push_or_replace_extension(&mut extensions, extension);
        }
    } else {
        for extension in get_enabled_extensions_with_config(config) {
            push_or_replace_extension(&mut extensions, extension);
        }
        for extension in crate::plugins::mcp_servers::enabled_plugin_mcp_servers(Some(project_root))
        {
            push_or_replace_extension(&mut extensions, extension);
        }
        add_mcp_servers(&mut extensions, mcp_servers)?;
    }

    Ok(extensions)
}

fn push_or_replace_extension(extensions: &mut Vec<ExtensionConfig>, extension: ExtensionConfig) {
    let name = extension.name().to_string();
    if let Some(index) = extensions
        .iter()
        .position(|existing| existing.name() == name)
    {
        extensions.remove(index);
    }
    extensions.push(extension);
}

fn resolve_default_provider_model_config(
    config: &Config,
) -> Result<(String, goose_providers::model::ModelConfig), agent_client_protocol::Error> {
    let resolved_provider = config.get_goose_provider().map_err(|error| {
        agent_client_protocol::Error::internal_error()
            .data(format!("Failed to resolve provider: {}", error))
    })?;
    let resolved_model = config.get_goose_model().map_err(|error| {
        agent_client_protocol::Error::internal_error()
            .data(format!("Failed to resolve model: {}", error))
    })?;
    let resolved_model_config =
        crate::model_config::model_config_from_user_config(&resolved_provider, &resolved_model)
            .map_err(|error| {
                agent_client_protocol::Error::internal_error()
                    .data(format!("Failed to resolve model: {}", error))
            })?;
    Ok((resolved_provider, resolved_model_config))
}

async fn resolve_provider_default_model_config(
    provider_name: &str,
) -> Result<goose_providers::model::ModelConfig, agent_client_protocol::Error> {
    let entry = crate::providers::get_from_registry(provider_name)
        .await
        .map_err(|error| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("Unknown provider '{}': {}", provider_name, error))
        })?;
    crate::model_config::model_config_from_user_config(
        provider_name,
        &entry.metadata().default_model,
    )
    .map_err(|error| {
        agent_client_protocol::Error::internal_error()
            .data(format!("Failed to resolve model: {}", error))
    })
}

fn render_resource_link(link: &ResourceLink) -> String {
    let inlined_file = Url::parse(&link.uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .and_then(|path| {
            let contents = fs::read_to_string(&path).ok()?;
            Some(format!(
                "\n\n# {}\n```\n{}\n```",
                path.to_string_lossy(),
                contents
            ))
        });

    inlined_file.unwrap_or_else(|| {
        let metadata = serde_json::json!({
            "name": link.name.as_str(),
            "uri": link.uri.as_str(),
        });
        format!("\n\n--- Resource link (not inlined) ---\n{metadata}\n---")
    })
}

fn rmcp_audience_annotations(annotations: Option<&Annotations>) -> Option<RmcpAnnotations> {
    let audience = annotations?
        .audience
        .as_ref()?
        .iter()
        .filter_map(|role| match role {
            agent_client_protocol::schema::v1::Role::Assistant => Some(Role::Assistant),
            agent_client_protocol::schema::v1::Role::User => Some(Role::User),
            _ => None,
        })
        .collect::<Vec<_>>();

    Some(RmcpAnnotations::default().with_audience(audience))
}

fn annotated_prompt_text(text: &str, annotations: Option<&Annotations>) -> RmcpTextContent {
    let content = RmcpTextContent::new(sanitize_unicode_tags(text));
    match rmcp_audience_annotations(annotations) {
        Some(annotations) => content.with_annotations(annotations),
        None => content,
    }
}

fn builtin_to_extension_config(name: &str) -> ExtensionConfig {
    if let Some(def) = PLATFORM_EXTENSIONS.get(name) {
        ExtensionConfig::Platform {
            name: def.name.into(),
            description: def.description.into(),
            display_name: Some(def.display_name.into()),
            bundled: Some(true),
            available_tools: vec![],
        }
    } else {
        ExtensionConfig::Builtin {
            name: name.into(),
            display_name: None,
            timeout: None,
            bundled: Some(true),
            description: name.into(),
            available_tools: vec![],
        }
    }
}

fn to_nonnegative_u64(value: Option<i32>) -> Option<u64> {
    value.and_then(|v| u64::try_from(v).ok())
}

fn build_prompt_usage(session: &Session) -> Option<Usage> {
    let total = to_nonnegative_u64(session.usage.total_tokens)?;
    let input = to_nonnegative_u64(session.usage.input_tokens).unwrap_or(0);
    let output = to_nonnegative_u64(session.usage.output_tokens).unwrap_or(0);
    Some(Usage::new(total, input, output))
}

fn prompt_stop_reason(was_cancelled: bool, output_token_limit_reached: bool) -> StopReason {
    if was_cancelled {
        StopReason::Cancelled
    } else if output_token_limit_reached {
        StopReason::MaxTokens
    } else {
        StopReason::EndTurn
    }
}

#[derive(Clone)]
struct SessionAgentTarget {
    agent: Arc<Agent>,
    session_id: String,
    cancel_token: Option<CancellationToken>,
}

struct PendingToolPermission {
    request_id: String,
    tool_name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
    prompt: Option<String>,
}

fn update_output_token_limit_reached(output_token_limit_reached: &mut bool, message: &Message) {
    if message.role == Role::Assistant {
        *output_token_limit_reached = message.metadata.output_token_limit_reached;
    }
}

pub(super) struct UsageUpdates {
    pub(super) custom: GooseSessionNotification,
    pub(super) standard: UsageUpdate,
}

pub(super) fn build_usage_updates(
    session: &Session,
    totals: &SessionUsageTotals,
    context_limit: usize,
) -> UsageUpdates {
    let used = session.usage.total_tokens.unwrap_or(0).max(0) as u64;
    let ctx_limit = context_limit as u64;
    let accumulated_input_tokens =
        to_nonnegative_u64(totals.accumulated_usage.input_tokens).unwrap_or(0);
    let accumulated_output_tokens =
        to_nonnegative_u64(totals.accumulated_usage.output_tokens).unwrap_or(0);
    UsageUpdates {
        custom: GooseSessionNotification {
            session_id: session.id.clone(),
            update: GooseSessionUpdate::UsageUpdate(SessionUsageUpdate {
                used,
                context_limit: ctx_limit,
                accumulated_input_tokens,
                accumulated_output_tokens,
                accumulated_cost: totals.accumulated_cost,
            }),
        },
        standard: {
            let mut standard = UsageUpdate::new(used, ctx_limit);
            if let Some(amount) = totals.accumulated_cost {
                standard = standard.cost(Cost::new(amount, "USD"));
            }
            standard
        },
    }
}

/// Resolve the cwd an existing session should be activated with: a
/// host-imposed cwd (roaming) wins, otherwise the client-requested cwd is
/// honored as-is, preserving standard ACP semantics.
pub(super) fn effective_session_cwd(host_cwd: Option<&Path>, requested: &Path) -> PathBuf {
    host_cwd.unwrap_or(requested).to_path_buf()
}

pub(super) fn validate_absolute_cwd(cwd: &Path) -> Result<(), agent_client_protocol::Error> {
    if !cwd.is_absolute() {
        return Err(
            agent_client_protocol::Error::invalid_params().data("cwd must be an absolute path")
        );
    }

    if !cwd.exists() || !cwd.is_dir() {
        return Err(agent_client_protocol::Error::invalid_params().data("invalid directory path"));
    }

    Ok(())
}

impl GooseAcpAgent {
    #[cfg(test)]
    pub(crate) fn active_run_registry(&self) -> &ActiveRunRegistry {
        &self.active_prompt_runs
    }

    #[cfg(test)]
    pub(crate) async fn test_start_active_run(
        &self,
        session_id: &str,
        run_id: String,
        agent: Arc<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        self.start_active_run(session_id, run_id, CancellationToken::new(), agent)
            .await
    }

    #[cfg(test)]
    pub(crate) fn test_drop_active_run_guard(&self, session_id: &str, run_id: &str) {
        drop(ActiveRunDropGuard {
            registry: self.active_prompt_runs.clone(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            cancel_token: CancellationToken::new(),
        });
    }

    #[cfg(test)]
    pub(crate) async fn test_require_active_run(
        &self,
        session_id: &str,
        expected_run_id: &str,
    ) -> Result<(String, Arc<Agent>), agent_client_protocol::Error> {
        self.require_active_run(session_id, expected_run_id).await
    }

    pub fn permission_manager(&self) -> Arc<PermissionManager> {
        Arc::clone(&self.permission_manager)
    }

    pub(super) fn supports_goose_custom_notifications(&self) -> bool {
        self.client_supports_goose_custom_notifications
            .get()
            .copied()
            .unwrap_or(false)
    }

    pub(super) async fn prepare_session_setup_by_id(
        &self,
        session_id: &str,
    ) -> Result<(Session, SessionUsageTotals, usize), agent_client_protocol::Error> {
        loop {
            let session = self
                .session_manager
                .get_session(session_id, false)
                .await
                .internal_err_ctx("Failed to load session for setup notifications")?;
            let model_name = session
                .model_config
                .as_ref()
                .map(|model| model.model_name.clone())
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("Session has no model")
                })?;
            let provider_name = session.provider_name.clone();
            let agent = self.get_session_agent(session_id).await?;
            let provider = agent
                .provider()
                .await
                .internal_err_ctx("Failed to resolve session provider")?;
            let context_limit =
                crate::context_limit::get_context_limit(provider.as_ref(), &model_name)
                    .await
                    .internal_err_ctx("Failed to resolve context limit")?;
            let session = self
                .session_manager
                .get_session(session_id, false)
                .await
                .internal_err_ctx("Failed to refresh session for setup notifications")?;
            let current_provider = agent
                .provider()
                .await
                .internal_err_ctx("Failed to refresh session provider")?;
            let refreshed_model_name = session
                .model_config
                .as_ref()
                .map(|model| model.model_name.as_str());
            if provider_name.as_deref() != Some(provider.get_name())
                || !Arc::ptr_eq(&provider, &current_provider)
                || session.provider_name != provider_name
                || refreshed_model_name != Some(model_name.as_str())
            {
                continue;
            }
            let totals = self
                .session_manager
                .get_session_usage_totals(session_id)
                .await
                .unwrap_or_default();
            return Ok((session, totals, context_limit));
        }
    }

    pub(super) fn supports_recipe_param_requests(&self) -> bool {
        self.client_supports_recipe_param_requests
            .get()
            .copied()
            .unwrap_or(false)
    }

    fn requests_tool_call_label_enrichment(&self) -> bool {
        self.client_requests_tool_call_label_enrichment
            .get()
            .copied()
            .unwrap_or(false)
    }

    fn supports_acp_elicitation(&self) -> bool {
        self.client_supports_acp_elicitation
            .get()
            .copied()
            .unwrap_or(false)
    }

    // TODO: goose reads Paths::in_state_dir globally (e.g. RequestLog), ignoring this data_dir.
    pub async fn new(options: GooseAcpAgentOptions) -> Result<Self> {
        let session_manager = Arc::new(SessionManager::new(options.data_dir));

        // Eagerly initialize the SQLite pool so it's ready when providers/sessions need it.
        let storage_clone = session_manager.storage().clone();
        tokio::spawn(async move {
            let _ = storage_clone.pool().await;
        });

        let permission_manager = Arc::new(PermissionManager::new(options.config_dir.clone()));
        let provider_inventory = ProviderInventoryService::new(session_manager.storage().clone());
        let agent_config = AgentConfig::new(
            Arc::clone(&session_manager),
            Arc::clone(&permission_manager),
            options.scheduler,
            Config::global().get_goose_mode().unwrap_or_default(),
            options.disable_session_naming,
            options.goose_platform.clone(),
        );
        let agent_manager = Arc::new(AgentManager::new(agent_config, None).await?);
        let (thinking_effort_update_tx, thinking_effort_update_rx) = mpsc::unbounded_channel();

        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            active_prompt_runs: options.active_prompt_runs,
            closed_session_ids: Arc::new(Mutex::new(HashSet::new())),
            agent_manager,
            provider_factory: options.provider_factory,
            builtin_selection: options.builtin_selection,
            client_fs_capabilities: OnceCell::new(),
            client_terminal: OnceCell::new(),
            client_mcp_host_info: OnceCell::new(),
            client_supports_acp_elicitation: OnceCell::new(),
            client_supports_goose_custom_notifications: OnceCell::new(),
            client_supports_recipe_param_requests: OnceCell::new(),
            client_requests_tool_call_label_enrichment: OnceCell::new(),
            use_login_shell_path: OnceCell::new(),
            client_cx: OnceCell::new(),
            thinking_effort_update_tx,
            thinking_effort_update_rx: Mutex::new(Some(thinking_effort_update_rx)),
            config_dir: options.config_dir,
            session_manager,
            permission_manager,
            disable_session_naming: options.disable_session_naming,
            provider_inventory,
            additional_source_roots: options.additional_source_roots,
            session_cwd: options.session_cwd,
            recipe_path_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn config(&self) -> Result<&'static Config, agent_client_protocol::Error> {
        Ok(Config::global())
    }

    async fn create_provider(
        &self,
        provider_name: &str,
        extensions: Vec<ExtensionConfig>,
        working_dir: Option<PathBuf>,
        use_default_model: bool,
    ) -> Result<Arc<dyn Provider>> {
        (self.provider_factory)(
            provider_name.to_string(),
            extensions,
            working_dir,
            use_default_model,
        )
        .await
    }

    /// Warm the provider model-list cache after session creation.
    ///
    /// This is a best-effort cache refresh, never a prerequisite for using the
    /// session, so it runs as a detached background task. Keeping it off the
    /// `session/new` critical path avoids stalling session creation on slow or
    /// blocking work such as a synchronous keychain read while resolving the
    /// provider's inventory identity.
    fn spawn_provider_inventory_refresh(&self, goose_session: &Session, agent: &Arc<Agent>) {
        let Some(provider_name) = goose_session.provider_name.clone() else {
            return;
        };
        let inventory_service = self.provider_inventory.clone();
        let agent = agent.clone();
        let session_id = goose_session.id.clone();
        tokio::spawn(async move {
            let Some(mut inventory) = inventory_service
                .find_entry_for_provider(&provider_name)
                .await
            else {
                return;
            };
            if !should_refresh_inventory_for_session_init(&inventory) {
                return;
            }
            let provider = match agent.provider().await {
                Ok(provider) => provider,
                Err(error) => {
                    warn!(
                        provider = %provider_name,
                        session = %session_id,
                        error = %error,
                        "agent has no provider available for inventory refresh"
                    );
                    return;
                }
            };
            inventory_service
                .refresh_with_provider(&provider_name, &provider, &mut inventory, "session init")
                .await;
        });
    }

    async fn get_or_create_session_agent_with_results(
        &self,
        cx: &ConnectionTo<Client>,
        session_id: String,
    ) -> Result<AgentManagerGetResult, agent_client_protocol::Error> {
        self.agent_manager
            .get_or_create_agent_with_runtime_context(
                session_id,
                RuntimeContext {
                    mcp_host_info: self.client_mcp_host_info.get().cloned(),
                    use_login_shell_path: self.use_login_shell_path.get().copied(),
                    session_name_update_tx: (!self.disable_session_naming)
                        .then(|| spawn_session_name_update_notifier(cx.clone())),
                },
            )
            .await
            .map_err(|error| agent_creation_error(error, "Failed to create agent"))
    }

    async fn apply_acp_extension_overrides(
        &self,
        cx: &ConnectionTo<Client>,
        agent: &Arc<Agent>,
        session: &Session,
    ) {
        let client_fs_capabilities = self
            .client_fs_capabilities
            .get()
            .cloned()
            .unwrap_or_default();
        let client_terminal = self.client_terminal.get().copied().unwrap_or(false);
        if !client_fs_capabilities.read_text_file
            && !client_fs_capabilities.write_text_file
            && !client_terminal
        {
            return;
        }

        if !agent
            .extension_manager
            .is_extension_enabled("developer")
            .await
        {
            return;
        }

        let context = agent.extension_manager.get_context().clone();
        let dev_client = match DeveloperClient::new(context) {
            Ok(dev_client) => dev_client,
            Err(error) => {
                warn!(error = %error, "Failed to create ACP developer client");
                return;
            }
        };

        let session_id = SessionId::new(session.id.clone());
        let client: Arc<dyn McpClientTrait> = Arc::new(AcpTools {
            inner: Arc::new(dev_client),
            cx: cx.clone(),
            session_id: session_id.clone(),
            tool_call_notifier: ToolCallNotifier::new(cx, &session_id),
            fs_read: client_fs_capabilities.read_text_file,
            fs_write: client_fs_capabilities.write_text_file,
            terminal: client_terminal,
        });
        let info = client.get_info().cloned();

        let developer_config = agent
            .extension_manager
            .get_extension_configs()
            .await
            .into_iter()
            .find(|extension| extension.name() == "developer")
            .unwrap_or_else(|| builtin_to_extension_config("developer"));

        agent
            .extension_manager
            .add_client("developer".into(), developer_config, client, info)
            .await;
    }

    async fn prepare_acp_session_agent(
        &self,
        cx: &ConnectionTo<Client>,
        session: &Session,
    ) -> Result<(Arc<Agent>, Vec<ExtensionLoadResult>), agent_client_protocol::Error> {
        let agent_result = self
            .get_or_create_session_agent_with_results(cx, session.id.clone())
            .await?;
        let agent = agent_result.agent.clone();
        self.apply_acp_extension_overrides(cx, &agent, session)
            .await;
        self.spawn_provider_inventory_refresh(session, &agent);

        Ok((agent, agent_result.extension_results))
    }

    async fn prepare_session_for_activation(
        &self,
        mut session: Session,
        cwd: std::path::PathBuf,
        mcp_servers: Vec<McpServer>,
        include_messages_on_reload: bool,
    ) -> Result<Session, agent_client_protocol::Error> {
        let config = Config::global();
        let mut builder = self.session_manager.update(&session.id);
        let mut session_needs_update = false;

        if cwd != session.working_dir {
            builder = builder.working_dir(cwd);
            session_needs_update = true;
        }

        if session.provider_name.is_none() || session.model_config.is_none() {
            let (resolved_provider, resolved_model_config) =
                resolve_default_provider_model_config(config)?;
            builder = builder
                .provider_name(resolved_provider)
                .model_config(resolved_model_config);
            session_needs_update = true;
        }

        if !mcp_servers.is_empty() {
            let mut stored_extensions =
                EnabledExtensionsState::from_extension_data(&session.extension_data)
                    .unwrap_or_else(|| EnabledExtensionsState::new(Vec::new()));
            add_mcp_servers(&mut stored_extensions.extensions, mcp_servers)?;
            builder = builder.extension_data(enabled_extensions_data(
                &session,
                stored_extensions.extensions,
            )?);
            session_needs_update = true;
        }

        if session_needs_update {
            let session_id = session.id.clone();
            builder
                .apply()
                .await
                .internal_err_ctx("Failed to update session")?;

            self.agent_manager
                .remove_session_if_loaded(&session_id)
                .await
                .internal_err_ctx("Failed to remove in-memory agent")?;

            session = self
                .session_manager
                .get_session(&session_id, include_messages_on_reload)
                .await
                .internal_err_ctx("Failed to reload session")?;
        }

        Ok(session)
    }

    fn build_enabled_extensions_data(
        &self,
        config: &Config,
        session: &Session,
        mcp_servers: Vec<McpServer>,
        goose_extensions: Option<Vec<GooseExtension>>,
        recipe_extensions: Option<&[ExtensionConfig]>,
    ) -> Result<ExtensionData, agent_client_protocol::Error> {
        let extensions = initial_session_extensions(
            config,
            &self.builtin_selection,
            &session.working_dir,
            mcp_servers,
            goose_extensions,
            recipe_extensions,
        )?;
        enabled_extensions_data(session, extensions)
    }

    async fn register_acp_session(&self, session_id: String, agent: Arc<Agent>) {
        let acp_session = GooseAcpSession {
            agent: agent.clone(),
        };
        self.sessions
            .lock()
            .await
            .insert(session_id.clone(), acp_session);
        self.subscribe_thinking_effort_updates(&session_id, &agent)
            .await;
    }

    async fn subscribe_thinking_effort_updates(&self, session_id: &str, agent: &Arc<Agent>) {
        let Ok(provider) = agent.provider().await else {
            return;
        };
        let Some(mut updates) = provider.subscribe_thinking_effort_support() else {
            return;
        };
        let session_id = session_id.to_string();
        let tx = self.thinking_effort_update_tx.clone();
        tokio::spawn(async move {
            while updates.changed().await.is_ok() {
                if tx.send(session_id.clone()).is_err() {
                    break;
                }
            }
        });
    }

    async fn start_thinking_effort_update_forwarder(self: &Arc<Self>, cx: &ConnectionTo<Client>) {
        let Some(mut updates) = self.thinking_effort_update_rx.lock().await.take() else {
            return;
        };
        let agent = Arc::downgrade(self);
        let cx = cx.clone();
        tokio::spawn(async move {
            while let Some(session_id) = updates.recv().await {
                let Some(agent) = agent.upgrade() else {
                    break;
                };
                if agent.closed_session_ids.lock().await.contains(&session_id) {
                    continue;
                }
                let session_id = SessionId::new(session_id);
                match agent.build_config_update(&session_id).await {
                    Ok((notification, _)) => {
                        if let Err(error) = cx.send_notification(notification) {
                            warn!(
                                session_id = %session_id,
                                %error,
                                "Failed to forward thinking-effort config update"
                            );
                        }
                    }
                    Err(error) => {
                        warn!(
                            session_id = %session_id,
                            ?error,
                            "Failed to build thinking-effort config update"
                        );
                    }
                }
            }
        });
    }

    async fn activate_acp_session(
        &self,
        cx: &ConnectionTo<Client>,
        session: &Session,
    ) -> Result<(Arc<Agent>, Vec<ExtensionLoadResult>), agent_client_protocol::Error> {
        let (agent, extension_results) = self.prepare_acp_session_agent(cx, session).await?;
        self.register_acp_session(session.id.clone(), agent.clone())
            .await;

        Ok((agent, extension_results))
    }

    pub async fn has_session(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains_key(session_id)
    }

    /// Convert ACP prompt content blocks into a user message.
    pub(crate) fn convert_acp_prompt_to_message(prompt: &[ContentBlock]) -> Message {
        let mut message = Message::user();
        for block in prompt {
            match block {
                ContentBlock::Text(text) => {
                    let annotated = annotated_prompt_text(&text.text, text.annotations.as_ref());
                    message = message.with_content(MessageContent::Text(annotated));
                }
                ContentBlock::Image(image) => {
                    let content = RmcpImageContent::new(&image.data, &image.mime_type);
                    let content = match rmcp_audience_annotations(image.annotations.as_ref()) {
                        Some(annotations) => content.with_annotations(annotations),
                        None => content,
                    };
                    message = message.with_content(MessageContent::Image(content));
                }
                ContentBlock::Resource(resource) => {
                    if let EmbeddedResourceResource::TextResourceContents(text_resource) =
                        &resource.resource
                    {
                        let header = format!("--- Resource: {} ---\n", text_resource.uri);
                        let content = format!("{}{}\n---\n", header, text_resource.text);
                        message = message.with_content(MessageContent::Text(
                            annotated_prompt_text(&content, resource.annotations.as_ref()),
                        ));
                    }
                }
                ContentBlock::ResourceLink(link) => {
                    let text = render_resource_link(link);
                    message = message.with_content(MessageContent::Text(annotated_prompt_text(
                        &text,
                        link.annotations.as_ref(),
                    )));
                }
                ContentBlock::Audio(..) | _ => (),
            }
        }
        message
    }

    async fn handle_message_content(
        &self,
        content_item: &MessageContent,
        message: &Message,
        session_id: &SessionId,
        target: &SessionAgentTarget,
        tool_requests: &HashMap<String, ToolRequest>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let role = &message.role;

        match content_item {
            MessageContent::Text(text) => {
                let chunk = content_chunk_for_message(
                    message,
                    ContentBlock::Text(TextContent::new(text.text.clone())),
                );
                let update = match role {
                    Role::User => SessionUpdate::UserMessageChunk(chunk),
                    Role::Assistant => SessionUpdate::AgentMessageChunk(chunk),
                };
                cx.send_notification(SessionNotification::new(session_id.clone(), update))?;
            }
            MessageContent::ToolRequest(tool_request) => {
                self.handle_tool_request(tool_request, message, session_id, &target.agent, cx)
                    .await?;
            }
            MessageContent::ToolResponse(tool_response) => {
                self.handle_tool_response(
                    tool_response,
                    tool_requests.get(&tool_response.id),
                    session_id,
                    cx,
                )
                .await?;
            }
            MessageContent::Thinking(thinking) => {
                cx.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentThoughtChunk(content_chunk_for_message(
                        message,
                        ContentBlock::Text(TextContent::new(thinking.thinking.clone())),
                    )),
                ))?;
            }
            MessageContent::ActionRequired(action_required) => match &action_required.data {
                ActionRequiredData::ToolConfirmation {
                    id,
                    tool_name,
                    arguments,
                    prompt,
                } => {
                    self.handle_tool_permission_request(
                        cx,
                        session_id,
                        PendingToolPermission {
                            request_id: id.clone(),
                            tool_name: tool_name.clone(),
                            arguments: arguments.clone(),
                            prompt: prompt.clone(),
                        },
                        target.clone(),
                    )?;
                }
                ActionRequiredData::Elicitation {
                    id,
                    message: elicitation_message,
                    requested_schema,
                } => {
                    self.handle_form_elicitation(
                        cx,
                        session_id,
                        id,
                        elicitation_message,
                        requested_schema,
                        message_meta_without_steer(message),
                    )
                    .await?;
                }
                ActionRequiredData::ElicitationResponse { .. } => {}
                ActionRequiredData::ToolConfirmationResponse { .. } => {}
            },
            MessageContent::Image(image) => {
                let mut image_content =
                    ImageContent::new(image.data.clone(), image.mime_type.clone());
                if let Some(audience) = image.annotations.as_ref().and_then(|a| a.audience.as_ref())
                {
                    image_content = image_content.annotations(
                        Annotations::new().audience(
                            audience
                                .iter()
                                .map(|r| match r {
                                    Role::Assistant => {
                                        agent_client_protocol::schema::v1::Role::Assistant
                                    }
                                    Role::User => agent_client_protocol::schema::v1::Role::User,
                                })
                                .collect::<Vec<_>>(),
                        ),
                    );
                }
                let chunk = content_chunk_for_message(message, ContentBlock::Image(image_content));
                let update = match role {
                    Role::User => SessionUpdate::UserMessageChunk(chunk),
                    Role::Assistant => SessionUpdate::AgentMessageChunk(chunk),
                };
                cx.send_notification(SessionNotification::new(session_id.clone(), update))?;
            }
            MessageContent::SystemNotification(notification) => {
                send_status_message_update(
                    cx,
                    self.supports_goose_custom_notifications(),
                    session_id.0.as_ref(),
                    notification,
                )?;
            }
            MessageContent::Error(error) => {
                let chunk = content_chunk_for_message(
                    message,
                    ContentBlock::Text(TextContent::new(error.message.clone())),
                );
                cx.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(chunk),
                ))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn spawn_ready_chain_summary(
        &self,
        chain: ReadyToolChain,
        agent: &Arc<Agent>,
        session_id: &SessionId,
        cx: &ConnectionTo<Client>,
    ) {
        if !self.requests_tool_call_label_enrichment() {
            return;
        }

        let tool_call_notifier = ToolCallNotifier::new(cx, session_id);
        spawn_chain_summary_enrichment(
            agent,
            session_id,
            tool_call_notifier,
            &self.session_manager,
            chain,
        );
    }

    async fn handle_tool_request(
        &self,
        tool_request: &ToolRequest,
        message: &Message,
        session_id: &SessionId,
        agent: &Arc<Agent>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let client_requests_label_enrichment = self.requests_tool_call_label_enrichment();
        let initial_tool_call = build_initial_tool_call_with_message_meta(
            tool_request,
            message,
            client_requests_label_enrichment,
        );
        let tool_call_notifier = ToolCallNotifier::new(cx, session_id);
        tool_call_notifier.send_initial(initial_tool_call)?;

        if !client_requests_label_enrichment {
            return Ok(());
        }

        if tool_request.tool_call.is_ok() {
            spawn_tool_title_enrichment(
                agent,
                tool_call_notifier,
                &self.session_manager,
                session_id.0.as_ref(),
                tool_request,
            );
        }

        Ok(())
    }

    async fn handle_tool_response(
        &self,
        tool_response: &ToolResponse,
        tool_request: Option<&ToolRequest>,
        session_id: &SessionId,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), agent_client_protocol::Error> {
        let fields = tool_call_update_fields_from_response(tool_response, tool_request, false);

        let update = ToolCallUpdate::new(ToolCallId::new(tool_response.id.clone()), fields)
            .meta(trusted_update_meta(tool_response));
        let tool_call_notifier = ToolCallNotifier::new(cx, session_id);
        tool_call_notifier.send_update(update)?;

        Ok(())
    }

    fn handle_tool_permission_request(
        &self,
        cx: &ConnectionTo<Client>,
        session_id: &SessionId,
        request: PendingToolPermission,
        target: SessionAgentTarget,
    ) -> Result<(), agent_client_protocol::Error> {
        let cx = cx.clone();
        let session_id = session_id.clone();

        let tool_call_update = build_permission_tool_call_update(
            &request.request_id,
            &request.tool_name,
            request.arguments,
            request.prompt,
        );

        fn option(kind: PermissionOptionKind) -> PermissionOption {
            let id = serde_json::to_value(kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            PermissionOption::new(id.clone(), id, kind)
        }
        let options = vec![
            option(PermissionOptionKind::AllowAlways),
            option(PermissionOptionKind::AllowOnce),
            option(PermissionOptionKind::RejectOnce),
            option(PermissionOptionKind::RejectAlways),
        ];

        let permission_request =
            RequestPermissionRequest::new(session_id, tool_call_update, options);
        let request_id = request.request_id;

        cx.send_request(permission_request)
            .on_receiving_result(move |result| async move {
                let permission = match result {
                    Ok(response) => outcome_to_confirmation(&response.outcome).permission,
                    Err(e) => {
                        error!(error = ?e, "permission request failed");
                        Permission::Cancel
                    }
                };

                if let Err(error) = target
                    .agent
                    .submit_tool_confirmation(&target.session_id, &request_id, permission)
                    .await
                {
                    error!(
                        session_id = %target.session_id,
                        request_id = %request_id,
                        %error,
                        "failed to submit tool confirmation"
                    );
                    if let Some(cancel_token) = target.cancel_token {
                        cancel_token.cancel();
                    }
                }
                Ok(())
            })?;

        Ok(())
    }
}

fn extract_client_supports_goose_custom_notifications(
    goose_client_capabilities: Option<&GooseClientCapabilities>,
) -> bool {
    goose_client_capabilities
        .and_then(|goose| goose.custom_notifications)
        .unwrap_or(false)
}

fn extract_client_supports_recipe_param_requests(
    goose_client_capabilities: Option<&GooseClientCapabilities>,
) -> bool {
    goose_client_capabilities
        .and_then(|goose| goose.recipe_parameter_requests)
        .unwrap_or(false)
}

fn outcome_to_confirmation(outcome: &RequestPermissionOutcome) -> PermissionConfirmation {
    PermissionConfirmation {
        principal_type: PrincipalType::Tool,
        permission: Permission::from(PermissionDecision::from(outcome)),
    }
}

fn prompt_error_from_message_content(
    content_item: &MessageContent,
) -> Option<agent_client_protocol::Error> {
    match content_item {
        MessageContent::Error(error)
            if error.kind == crate::conversation::message::MessageErrorKind::Authentication =>
        {
            Some(agent_client_protocol::Error::auth_required())
        }
        MessageContent::SystemNotification(notification)
            if notification.notification_type == SystemNotificationType::CreditsExhausted =>
        {
            Some(credits_exhausted_prompt_error(notification))
        }
        MessageContent::Error(error)
            if error.kind == crate::conversation::message::MessageErrorKind::CreditsExhausted =>
        {
            let mut data = serde_json::Map::new();
            data.insert(
                "reason".to_string(),
                serde_json::Value::String(crate::acp::CREDITS_EXHAUSTED_REASON.to_string()),
            );
            Some(
                agent_client_protocol::Error::new(-32603, error.message.clone())
                    .data(serde_json::Value::Object(data)),
            )
        }
        _ => None,
    }
}

fn credits_exhausted_prompt_error(
    notification: &SystemNotificationContent,
) -> agent_client_protocol::Error {
    let mut data = serde_json::Map::new();
    data.insert(
        "reason".to_string(),
        serde_json::Value::String(crate::acp::CREDITS_EXHAUSTED_REASON.to_string()),
    );

    if let Some(url) = notification
        .data
        .as_ref()
        .and_then(|data| data.get("top_up_url"))
        .and_then(|url| url.as_str())
    {
        data.insert(
            "url".to_string(),
            serde_json::Value::String(url.to_string()),
        );
    }

    agent_client_protocol::Error::new(-32603, notification.msg.clone())
        .data(serde_json::Value::Object(data))
}

fn send_status_message_update(
    cx: &ConnectionTo<Client>,
    supports_goose_custom_notifications: bool,
    session_id: &str,
    notification: &SystemNotificationContent,
) -> Result<(), agent_client_protocol::Error> {
    if let Some(status) = status_message_from_system_notification(notification) {
        if supports_goose_custom_notifications {
            cx.send_notification(GooseSessionNotification {
                session_id: session_id.to_string(),
                update: GooseSessionUpdate::StatusMessage(StatusMessageUpdate { status }),
            })?;
        }
    }
    Ok(())
}

fn send_progress_message_update(
    cx: &ConnectionTo<Client>,
    supports_goose_custom_notifications: bool,
    session_id: &str,
    message: String,
) -> Result<(), agent_client_protocol::Error> {
    if supports_goose_custom_notifications {
        cx.send_notification(GooseSessionNotification {
            session_id: session_id.to_string(),
            update: GooseSessionUpdate::StatusMessage(StatusMessageUpdate {
                status: StatusMessage::Progress { message },
            }),
        })?;
    }
    Ok(())
}

fn status_message_from_system_notification(
    notification: &SystemNotificationContent,
) -> Option<StatusMessage> {
    match notification.notification_type {
        SystemNotificationType::InlineMessage => Some(StatusMessage::Notice {
            message: notification.msg.clone(),
        }),
        SystemNotificationType::ThinkingMessage | SystemNotificationType::ProgressMessage => {
            Some(StatusMessage::Progress {
                message: notification.msg.clone(),
            })
        }
        SystemNotificationType::CreditsExhausted => None,
    }
}

/// Conversion to the sdk-types wire mirror carried by `message_usage`.
fn message_usage_update(
    message_id: Option<String>,
    usage: &crate::conversation::message::MessageUsage,
) -> MessageUsageUpdate {
    use crate::conversation::token_usage::CostSource;

    MessageUsageUpdate {
        message_id,
        usage: MessageUsageData {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            cost: usage.cost,
            cost_source: usage.cost_source.map(|source| match source {
                CostSource::ProviderReported => CostSourceData::ProviderReported,
                CostSource::Estimated => CostSourceData::Estimated,
            }),
            elapsed_ms: usage.elapsed_ms,
            time_to_first_token_ms: usage.time_to_first_token_ms,
            is_compaction: usage.is_compaction,
        },
    }
}

impl GooseAcpAgent {
    async fn on_initialize(
        &self,
        args: InitializeRequest,
    ) -> Result<InitializeResponse, agent_client_protocol::Error> {
        debug!(?args, "initialize request");

        let _ = self
            .client_fs_capabilities
            .set(args.client_capabilities.fs.clone());
        let _ = self.client_terminal.set(args.client_capabilities.terminal);
        let goose_client_capabilities =
            extract_client_capabilities_meta(&args).and_then(|meta| meta.goose);
        let _ = self.client_mcp_host_info.set(extract_client_mcp_host_info(
            &args,
            goose_client_capabilities.as_ref(),
        ));
        let _ = self.client_supports_goose_custom_notifications.set(
            extract_client_supports_goose_custom_notifications(goose_client_capabilities.as_ref()),
        );
        let _ = self.client_supports_recipe_param_requests.set(
            extract_client_supports_recipe_param_requests(goose_client_capabilities.as_ref()),
        );
        let client_requests_tool_call_label_enrichment = goose_client_capabilities
            .as_ref()
            .and_then(|goose| goose.tool_call_label_enrichment)
            .unwrap_or(false);
        let _ = self
            .client_requests_tool_call_label_enrichment
            .set(client_requests_tool_call_label_enrichment);
        let _ = self
            .client_supports_acp_elicitation
            .set(elicitation::client_supports_form_elicitation(&args));
        let _ = self
            .use_login_shell_path
            .set(extract_use_login_shell_path(&args));

        let capabilities = AgentCapabilities::new()
            .load_session(true)
            .session_capabilities(
                SessionCapabilities::new()
                    .list(SessionListCapabilities::new())
                    .delete(SessionDeleteCapabilities::new())
                    .close(SessionCloseCapabilities::new()),
            )
            .prompt_capabilities(
                PromptCapabilities::new()
                    .image(true)
                    .audio(false)
                    .embedded_context(true),
            )
            .mcp_capabilities(McpCapabilities::new().http(true))
            .meta(agent_capabilities_meta());
        Ok(InitializeResponse::new(args.protocol_version)
            .agent_info(Implementation::new("goose", env!("CARGO_PKG_VERSION")))
            .agent_capabilities(capabilities)
            .auth_methods(vec![AuthMethod::Agent(
                AuthMethodAgent::new("goose-provider", "Configure Provider")
                    .description("Run `goose configure` to set up your AI provider and API key"),
            )]))
    }

    async fn on_new_session(
        &self,
        cx: &ConnectionTo<Client>,
        args: NewSessionRequest,
    ) -> Result<NewSessionResponse, agent_client_protocol::Error> {
        self.handle_new_session(cx, args).await
    }

    /// Look up the session's agent.
    async fn get_session_agent(
        &self,
        session_id: &str,
    ) -> Result<Arc<Agent>, agent_client_protocol::Error> {
        if self.closed_session_ids.lock().await.contains(session_id) {
            return Err(agent_client_protocol::Error::resource_not_found(Some(
                session_id.to_string(),
            ))
            .data(format!("Session not found: {}", session_id)));
        }

        {
            let sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get(session_id) {
                return Ok(session.agent.clone());
            }
        }

        let cx = self.client_cx.get().ok_or_else(|| {
            agent_client_protocol::Error::resource_not_found(Some(session_id.to_string()))
                .data(format!("Session not found: {}", session_id))
        })?;
        let session = self
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|_| {
                agent_client_protocol::Error::resource_not_found(Some(session_id.to_string()))
                    .data(format!("Session not found: {}", session_id))
            })?;
        let (agent, _) = self.activate_acp_session(cx, &session).await?;
        Ok(agent)
    }

    async fn start_active_run(
        &self,
        session_id: &str,
        run_id: String,
        cancel_token: CancellationToken,
        agent: Arc<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        if self.closed_session_ids.lock().await.contains(session_id) {
            return Err(agent_client_protocol::Error::resource_not_found(Some(
                session_id.to_string(),
            ))
            .data(format!("Session not found: {}", session_id)));
        }

        let mut active_prompt_runs = self.active_prompt_runs.lock().await;
        if let Some(active_run) = active_prompt_runs.get(session_id) {
            return Err(agent_client_protocol::Error::invalid_params().data(format!(
                "session already has active run `{}`; use _goose/unstable/session/steer",
                active_run.run_id.as_str()
            )));
        }

        active_prompt_runs.insert(
            session_id.to_string(),
            ActivePromptRun {
                run_id,
                cancel_token,
                agent,
            },
        );
        Ok(())
    }

    async fn clear_active_run(&self, session_id: &str, run_id: &str) {
        let agent = {
            let mut active_prompt_runs = self.active_prompt_runs.lock().await;
            let Some(active_run) = active_prompt_runs.get(session_id) else {
                return;
            };

            if active_run.run_id != run_id {
                return;
            }

            active_prompt_runs
                .remove(session_id)
                .map(|active_run| active_run.agent)
        };

        // Discard steers on the agent that owned the run; under roaming it may
        // not be this connection's agent.
        if let Some(agent) = agent {
            agent.discard_pending_steers(session_id).await;
        }

        if self.closed_session_ids.lock().await.contains(session_id) {
            self.sessions.lock().await.remove(session_id);
            if let Err(error) = self
                .agent_manager
                .remove_session_if_loaded(session_id)
                .await
            {
                tracing::warn!(
                    session_id,
                    %error,
                    "Failed to remove in-memory agent for closed session"
                );
            }
        }
    }

    async fn require_active_run(
        &self,
        session_id: &str,
        expected_run_id: &str,
    ) -> Result<(String, Arc<Agent>), agent_client_protocol::Error> {
        if expected_run_id.is_empty() {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("expectedRunId must not be empty"));
        }

        let active_prompt_runs = self.active_prompt_runs.lock().await;
        let active_run = active_prompt_runs.get(session_id).ok_or_else(|| {
            agent_client_protocol::Error::invalid_params().data("no active run to steer")
        })?;
        if active_run.run_id != expected_run_id {
            return Err(
                agent_client_protocol::Error::invalid_params().data(serde_json::json!({
                    "message": format!(
                        "expected active run id `{expected_run_id}` but found `{}`",
                        active_run.run_id.as_str()
                    ),
                    "expectedRunId": expected_run_id,
                    "actualRunId": active_run.run_id.as_str(),
                })),
            );
        }
        Ok((active_run.run_id.clone(), active_run.agent.clone()))
    }

    fn active_run_meta(active_run_id: Option<&str>) -> Meta {
        let mut goose = serde_json::Map::new();
        goose.insert(
            "activeRunId".to_string(),
            active_run_id
                .map(|run_id| serde_json::Value::String(run_id.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );

        let mut meta = serde_json::Map::new();
        meta.insert("goose".to_string(), serde_json::Value::Object(goose));
        meta
    }

    fn send_active_run_update(
        cx: &ConnectionTo<Client>,
        session_id: &SessionId,
        active_run_id: Option<&str>,
    ) -> Result<(), agent_client_protocol::Error> {
        cx.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().meta(Self::active_run_meta(active_run_id)),
            ),
        ))
    }

    fn send_queued_steer_update(
        cx: &ConnectionTo<Client>,
        session_id: &SessionId,
        message_id: &str,
        run_id: &str,
    ) -> Result<(), agent_client_protocol::Error> {
        let mut goose = serde_json::Map::new();
        goose.insert(
            "queuedSteer".to_string(),
            serde_json::json!({
                "messageId": message_id,
                "runId": run_id,
            }),
        );
        let mut meta = serde_json::Map::new();
        meta.insert("goose".to_string(), serde_json::Value::Object(goose));

        cx.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(meta)),
        ))
    }

    async fn send_local_inference_progress_update(
        &self,
        cx: &ConnectionTo<Client>,
        acp_session_id: &SessionId,
        session_id: &str,
        agent: &Arc<Agent>,
    ) -> Result<(), agent_client_protocol::Error> {
        let Ok(provider) = agent.provider().await else {
            return Ok(());
        };
        if provider.get_name() != "local" {
            return Ok(());
        }

        let model_config = agent.model_config_for_session(session_id).await.ok();
        let model_name = model_config
            .as_ref()
            .map(|config| config.model_name.clone())
            .unwrap_or_else(|| "local model".to_string());

        #[cfg(feature = "local-inference")]
        if let Some(model_config) = model_config.as_ref() {
            if crate::providers::local_inference::is_model_loaded(&model_config.model_name)
                .await
                .unwrap_or(false)
            {
                return Ok(());
            }
        }

        send_progress_message_update(
            cx,
            self.supports_goose_custom_notifications(),
            acp_session_id.0.as_ref(),
            format!("Loading local model {model_name}..."),
        )
    }

    async fn send_session_usage_updates(
        &self,
        cx: &ConnectionTo<Client>,
        acp_session_id: &SessionId,
        session_id: &str,
        agent: &Arc<Agent>,
    ) -> Result<Session, agent_client_protocol::Error> {
        let session = self
            .session_manager
            .get_session(session_id, false)
            .await
            .internal_err_ctx("Failed to load session")?;
        let totals = self
            .session_manager
            .get_session_usage_totals(session_id)
            .await
            .unwrap_or_default();
        let provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to resolve session provider")?;
        let model = session.model_config.as_ref().ok_or_else(|| {
            agent_client_protocol::Error::internal_error().data("Session has no model")
        })?;
        let context_limit =
            crate::context_limit::get_context_limit(provider.as_ref(), &model.model_name)
                .await
                .internal_err_ctx("Failed to resolve context limit")?;
        let updates = build_usage_updates(&session, &totals, context_limit);
        if self.supports_goose_custom_notifications() {
            cx.send_notification(updates.custom)?;
        }
        cx.send_notification(SessionNotification::new(
            acp_session_id.clone(),
            SessionUpdate::UsageUpdate(updates.standard),
        ))?;

        Ok(session)
    }

    async fn forward_agent_stream(
        &self,
        cx: &ConnectionTo<Client>,
        acp_session_id: &SessionId,
        session_id: &str,
        agent: &Arc<Agent>,
        cancel_token: &CancellationToken,
        mut stream: BoxStream<'_, Result<crate::agents::AgentEvent>>,
    ) -> Result<AgentStreamOutcome, agent_client_protocol::Error> {
        let mut was_cancelled = false;
        let mut output_token_limit_reached = false;
        let mut tool_requests = HashMap::new();
        let mut chain_tracker = ToolChainTracker::default();
        let target = SessionAgentTarget {
            agent: agent.clone(),
            session_id: session_id.to_string(),
            cancel_token: Some(cancel_token.clone()),
        };

        while let Some(event) = stream.next().await {
            if cancel_token.is_cancelled() {
                was_cancelled = true;
                break;
            }

            match event {
                Ok(crate::agents::AgentEvent::Message(mut message)) => {
                    update_output_token_limit_reached(&mut output_token_limit_reached, &message);

                    let sessions = self.sessions.lock().await;
                    if !sessions.contains_key(session_id) {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("Session not found: {session_id}")));
                    }
                    drop(sessions);

                    populate_output_token_limit_content(&mut message);
                    for content_item in &message.content {
                        if let Some(error) = prompt_error_from_message_content(content_item) {
                            return Err(error);
                        }

                        if let MessageContent::ToolRequest(tool_request) = content_item {
                            tool_requests.insert(tool_request.id.clone(), tool_request.clone());
                        }

                        self.handle_message_content(
                            content_item,
                            &message,
                            acp_session_id,
                            &target,
                            &tool_requests,
                            cx,
                        )
                        .await?;

                        let ready_chain = match content_item {
                            MessageContent::ToolRequest(tool_request) => {
                                chain_tracker.record_request(tool_request.clone());
                                None
                            }
                            MessageContent::ToolResponse(tool_response) => {
                                chain_tracker.record_response(&tool_response.id)
                            }
                            content if breaks_consecutive_tool_calls(content) => {
                                chain_tracker.close_current_chain()
                            }
                            _ => None,
                        };

                        if let Some(chain) = ready_chain {
                            self.spawn_ready_chain_summary(chain, agent, acp_session_id, cx);
                        }
                    }
                }
                Ok(crate::agents::AgentEvent::McpNotification((request_id, notification))) => {
                    if let Some(update) =
                        tool_notifications::tool_notification_update(request_id, notification)
                    {
                        let tool_call_notifier = ToolCallNotifier::new(cx, acp_session_id);
                        tool_call_notifier.send_update(update)?;
                    }
                }
                Ok(crate::agents::AgentEvent::MessageUsage { message_id, usage }) => {
                    if self.supports_goose_custom_notifications() {
                        cx.send_notification(GooseSessionNotification {
                            session_id: session_id.to_string(),
                            update: GooseSessionUpdate::MessageUsage(message_usage_update(
                                message_id, &usage,
                            )),
                        })?;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(agent_client_protocol::Error::internal_error()
                        .data(format!("Error in agent response stream: {error}")));
                }
            }
        }

        if cancel_token.is_cancelled() {
            was_cancelled = true;
        }

        if !was_cancelled {
            if let Some(chain) = chain_tracker.close_current_chain() {
                self.spawn_ready_chain_summary(chain, agent, acp_session_id, cx);
            }
        }

        Ok(AgentStreamOutcome {
            was_cancelled,
            output_token_limit_reached,
        })
    }

    async fn on_load_session(
        self: &Arc<Self>,
        cx: &ConnectionTo<Client>,
        args: LoadSessionRequest,
    ) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
        self.handle_load_session(cx, args).await
    }

    async fn on_prompt(
        &self,
        cx: &ConnectionTo<Client>,
        args: PromptRequest,
    ) -> Result<PromptResponse, agent_client_protocol::Error> {
        // The ACP session_id IS the thread ID.
        let session_id = args.session_id.0.to_string();

        let run_id = format!("run_{}", Uuid::new_v4());
        let cancel_token = CancellationToken::new();

        // Resolve the agent before claiming the run so the registry can record
        // which agent owns it; registration stays atomic, so the cross-connection
        // guard still admits only one run per session.
        let agent = self.get_session_agent(&session_id).await?;
        self.start_active_run(
            &session_id,
            run_id.clone(),
            cancel_token.clone(),
            agent.clone(),
        )
        .await?;

        // Frees the run if this future is dropped mid-prompt (e.g. the roaming
        // connection carrying it is revoked or lost); a normal completion's
        // explicit clear wins and makes the guard's cleanup a no-op.
        let _run_guard = ActiveRunDropGuard {
            registry: self.active_prompt_runs.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            cancel_token: cancel_token.clone(),
        };

        if cancel_token.is_cancelled() {
            self.clear_active_run(&session_id, &run_id).await;
            Self::send_active_run_update(cx, &args.session_id, None)?;
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }

        if let Err(error) = Self::send_active_run_update(cx, &args.session_id, Some(&run_id)) {
            self.clear_active_run(&session_id, &run_id).await;
            return Err(error);
        }

        if let Err(error) = self
            .send_local_inference_progress_update(cx, &args.session_id, &session_id, &agent)
            .await
        {
            self.clear_active_run(&session_id, &run_id).await;
            let _ = Self::send_active_run_update(cx, &args.session_id, None);
            return Err(error);
        }

        let user_message = Self::convert_acp_prompt_to_message(&args.prompt);
        let session_config = SessionConfig {
            id: session_id.clone(),
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };

        let stream = match agent
            .reply(user_message, session_config, Some(cancel_token.clone()))
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                self.clear_active_run(&session_id, &run_id).await;
                let _ = Self::send_active_run_update(cx, &args.session_id, None);
                return Err(agent_client_protocol::Error::internal_error()
                    .data(format!("Error getting agent reply: {error}")));
            }
        };
        let stream_result = self
            .forward_agent_stream(
                cx,
                &args.session_id,
                &session_id,
                &agent,
                &cancel_token,
                stream,
            )
            .await;
        self.clear_active_run(&session_id, &run_id).await;
        Self::send_active_run_update(cx, &args.session_id, None)?;
        let outcome = stream_result?;

        let session = self
            .send_session_usage_updates(cx, &args.session_id, &session_id, &agent)
            .await?;

        let stop_reason =
            prompt_stop_reason(outcome.was_cancelled, outcome.output_token_limit_reached);

        let mut response = PromptResponse::new(stop_reason);
        if let Some(usage) = build_prompt_usage(&session) {
            response = response.usage(usage);
        }
        Ok(response)
    }

    async fn on_steer_session(
        &self,
        req: SteerSessionRequest,
    ) -> Result<SteerSessionResponse, agent_client_protocol::Error> {
        if req.prompt.is_empty() {
            return Err(
                agent_client_protocol::Error::invalid_params().data("prompt must not be empty")
            );
        }

        // Route to the agent that owns the run, not this connection's agent:
        // under roaming the steering client may be a different connection than
        // the one running the prompt.
        let (active_run_id, agent) = self
            .require_active_run(&req.session_id, &req.expected_run_id)
            .await?;

        let message = Self::convert_acp_prompt_to_message(&req.prompt);
        if message.content.is_empty() {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("prompt must contain steerable content"));
        }

        let message_id = format!("steer_{}", Uuid::new_v4());
        let message = message.with_id(message_id.clone());
        agent.steer(&req.session_id, message).await;

        if let Some(cx) = self.client_cx.get() {
            let _ = Self::send_queued_steer_update(
                cx,
                &SessionId::new(req.session_id.clone()),
                &message_id,
                &active_run_id,
            );
        }

        Ok(SteerSessionResponse {
            run_id: active_run_id,
            message_id,
        })
    }

    async fn on_cancel(
        &self,
        args: CancelNotification,
    ) -> Result<(), agent_client_protocol::Error> {
        debug!(?args, "cancel request");

        let session_id = args.session_id.0.to_string();
        let token = {
            let active_prompt_runs = self.active_prompt_runs.lock().await;
            active_prompt_runs
                .get(&session_id)
                .map(|active_run| active_run.cancel_token.clone())
        };

        if let Some(token) = token {
            info!(session_id = %session_id, "prompt cancelled");
            token.cancel();
        } else if !self.sessions.lock().await.contains_key(&session_id) {
            warn!(session_id = %session_id, "cancel request for unknown session");
        }

        Ok(())
    }

    async fn on_set_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<(), agent_client_protocol::Error> {
        let agent = self.get_session_agent(session_id).await?;
        let current_provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to get provider")?;
        let provider_name = current_provider.get_name().to_string();
        let current_model_config = agent
            .model_config_for_session(session_id)
            .await
            .internal_err_ctx("Failed to resolve model config")?;
        let model_config =
            crate::model_config::model_config_from_user_config_with_session_settings(
                &provider_name,
                model_id,
                Some(&current_model_config),
                None,
                None,
            )
            .invalid_params_err_ctx("Invalid model config")?;
        agent
            .recreate_provider_for_session(session_id, &provider_name, model_config)
            .await
            .internal_err_ctx("Failed to recreate provider")?;
        self.subscribe_thinking_effort_updates(session_id, &agent)
            .await;
        // model_config is already updated on the session by the agent's update_provider call.
        Ok(())
    }

    async fn build_config_update(
        &self,
        session_id: &SessionId,
    ) -> Result<(SessionNotification, Vec<SessionConfigOption>), agent_client_protocol::Error> {
        let session = self
            .session_manager
            .get_session(&session_id.0, false)
            .await
            .internal_err()?;
        let agent = self.get_session_agent(&session_id.0).await?;
        let provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to get provider")?;
        let provider_name = provider.get_name().to_string();
        let current_model_config = agent
            .model_config_for_session(&session_id.0)
            .await
            .internal_err_ctx("Failed to resolve model config")?;
        let current_model = current_model_config.model_name.clone();
        let goose_mode = agent.goose_mode().await;
        let inventory = self
            .provider_inventory
            .entry_for_provider(&provider_name)
            .await
            .internal_err()?;
        let Some(inventory) = inventory else {
            return Err(agent_client_protocol::Error::internal_error()
                .data(format!("Unknown provider inventory: {}", provider_name)));
        };
        let model_state = build_model_state(current_model.as_str(), &inventory);
        let mode_state = build_mode_state(goose_mode)?;
        let provider_options = build_provider_options(Some(&provider_name)).await;
        let config_options = build_config_options(
            &mode_state,
            &model_state,
            &current_model_config,
            session_provider_selection(&session),
            provider_options,
            &provider.thinking_effort_support(),
        );
        let notification = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options.clone())),
        );
        Ok((notification, config_options))
    }

    async fn on_set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<SetSessionModeResponse, agent_client_protocol::Error> {
        let mode = mode_id.parse::<GooseMode>().map_err(|_| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("Invalid mode: {}", mode_id))
        })?;

        let agent = self.get_session_agent(session_id).await?;
        agent
            .update_goose_mode(mode, session_id)
            .await
            .internal_err_ctx("Failed to update mode")?;

        // goose_mode is already updated on the session above.

        Ok(SetSessionModeResponse::new())
    }

    async fn on_set_thinking_effort(
        &self,
        session_id: &str,
        effort_id: &str,
    ) -> Result<(), agent_client_protocol::Error> {
        let agent = self.get_session_agent(session_id).await?;
        agent
            .update_thinking_effort(session_id, effort_id)
            .await
            .map_err(thinking_effort_error)?;

        Ok(())
    }

    async fn update_provider(
        &self,
        session_id: &str,
        provider_name: &str,
        model_name: Option<&str>,
        context_limit: Option<usize>,
        request_params: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<(), agent_client_protocol::Error> {
        let config = self.config()?;
        let agent = self.get_session_agent(session_id).await?;
        let current_provider = agent
            .provider()
            .await
            .internal_err_ctx("Failed to get provider")?;
        let current_provider_name = current_provider.get_name();
        let current_model_config = agent
            .model_config_for_session(session_id)
            .await
            .internal_err_ctx("Failed to resolve model config")?;
        let current_model = current_model_config.model_name.clone();
        let use_default_provider = provider_name == DEFAULT_PROVIDER_ID;
        let resolved_provider_name = if use_default_provider {
            config
                .get_goose_provider()
                .internal_err_ctx("Failed to resolve default provider from config")?
        } else {
            provider_name.to_string()
        };
        let is_changing_provider = resolved_provider_name != current_provider_name;
        let default_model = if let Some(model_name) = model_name {
            model_name.to_string()
        } else if use_default_provider {
            config
                .get_goose_model()
                .internal_err_ctx("Failed to resolve default model from config")?
        } else if is_changing_provider {
            crate::providers::get_from_registry(&resolved_provider_name)
                .await
                .ok()
                .map(|entry| entry.metadata().default_model.clone())
                .unwrap_or(ACP_CURRENT_MODEL.to_string())
        } else {
            current_model
        };
        let model = model_name.unwrap_or(&default_model);
        let model_config =
            crate::model_config::model_config_from_user_config_with_session_settings(
                &resolved_provider_name,
                model,
                Some(&current_model_config),
                request_params,
                context_limit,
            )
            .invalid_params_err_ctx("Invalid model config")?;

        agent
            .recreate_provider_for_session(session_id, &resolved_provider_name, model_config)
            .await
            .internal_err_ctx("Failed to recreate provider")?;
        self.subscribe_thinking_effort_updates(session_id, &agent)
            .await;

        // provider_name is already updated on the session by the agent's update_provider call.
        Ok(())
    }

    async fn on_fork_session(
        &self,
        cx: &ConnectionTo<Client>,
        args: ForkSessionRequest,
    ) -> Result<ForkSessionResponse, agent_client_protocol::Error> {
        self.handle_fork_session(cx, args).await
    }

    async fn on_close_session(
        &self,
        session_id: &str,
    ) -> Result<CloseSessionResponse, agent_client_protocol::Error> {
        self.closed_session_ids
            .lock()
            .await
            .insert(session_id.to_string());

        let active_run_token = {
            let active_prompt_runs = self.active_prompt_runs.lock().await;
            active_prompt_runs
                .get(session_id)
                .map(|active_run| active_run.cancel_token.clone())
        };

        if let Some(token) = active_run_token {
            token.cancel();
        }

        let mut sessions = self.sessions.lock().await;
        sessions.remove(session_id);
        drop(sessions);

        self.agent_manager
            .remove_session_if_loaded(session_id)
            .await
            .internal_err_ctx("Failed to remove in-memory agent")?;

        info!(session_id = %session_id, "ACP session closed");
        Ok(CloseSessionResponse::new())
    }
}

pub struct GooseAcpHandler {
    pub agent: Arc<GooseAcpAgent>,
}

pub fn serve<R, W>(
    agent: Arc<GooseAcpAgent>,
    read: R,
    write: W,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
where
    R: futures::AsyncRead + Unpin + Send + 'static,
    W: futures::AsyncWrite + Unpin + Send + 'static,
{
    Box::pin(async move {
        let handler = GooseAcpHandler { agent };

        SacpAgent
            .builder()
            .name("goose-acp")
            .with_handler(handler)
            .connect_to(ByteStreams::new(write, read))
            .await?;

        Ok(())
    })
}

/// A lazily-initialized agent connection used by the HTTP/WebSocket transport.
///
/// The `agent-client-protocol-http` server takes a synchronous factory that
/// yields a [`ConnectTo<Client>`] per connection, but creating a goose agent is
/// async. Agent creation is therefore deferred into [`ConnectTo::connect_to`],
/// which runs as the connection's serving future.
pub struct GooseAgentConnection {
    server: Arc<crate::acp::server_factory::AcpServer>,
}

impl GooseAgentConnection {
    pub fn new(server: Arc<crate::acp::server_factory::AcpServer>) -> Self {
        Self { server }
    }
}

impl agent_client_protocol::ConnectTo<Client> for GooseAgentConnection {
    async fn connect_to(
        self,
        client: impl agent_client_protocol::ConnectTo<SacpAgent>,
    ) -> std::result::Result<(), agent_client_protocol::Error> {
        let agent = self.server.create_agent().await.internal_err()?;
        let handler = GooseAcpHandler { agent };
        SacpAgent
            .builder()
            .name("goose-acp")
            .with_handler(handler)
            .connect_to(client)
            .await
    }
}

pub async fn run(builtins: Vec<String>, enable_scheduler: bool) -> Result<()> {
    info!("listening on stdio");

    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    let server = crate::acp::server_factory::AcpServer::new(
        crate::acp::server_factory::AcpServerFactoryConfig {
            builtins: AcpBuiltinSelection::from_requested(builtins),
            data_dir: Paths::data_dir(),
            config_dir: Paths::config_dir(),
            goose_platform: GoosePlatform::GooseCli,
            additional_source_roots: Vec::new(),
            session_cwd: None,
            enable_scheduler,
        },
    );
    let agent = server.create_agent().await?;
    serve(agent, incoming, outgoing).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::session_manager::SessionType;
    use agent_client_protocol::schema::v1::{
        EmbeddedResource, EnvVariable, HttpHeader, McpServer, McpServerHttp, McpServerSse,
        McpServerStdio, PermissionOptionId, ResourceLink, Role as AcpRole,
        SelectedPermissionOutcome, TextResourceContents,
    };
    use goose_providers::conversation::token_usage::Usage as TokenUsage;
    use goose_providers::thinking::{
        ThinkingEffortCapability, ThinkingEffortOption, ThinkingEffortSupport,
    };
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use test_case::test_case;

    #[derive(Debug)]
    struct AsyncEffortProvider {
        updates: tokio::sync::watch::Sender<ThinkingEffortSupport>,
    }

    impl AsyncEffortProvider {
        fn new() -> Self {
            let (updates, _) = tokio::sync::watch::channel(Self::support("low", &["low", "high"]));
            Self { updates }
        }

        fn support(current: &str, values: &[&str]) -> ThinkingEffortSupport {
            ThinkingEffortSupport::Options(ThinkingEffortCapability {
                option_id: "effort".to_string(),
                values: values
                    .iter()
                    .map(|value| ThinkingEffortOption {
                        value: value.to_string(),
                        label: value.to_string(),
                    })
                    .collect(),
                current: Some(current.to_string()),
            })
        }

        fn update(&self, current: &str, values: &[&str]) {
            self.updates.send_replace(Self::support(current, values));
        }
    }

    #[async_trait::async_trait]
    impl Provider for AsyncEffortProvider {
        fn get_name(&self) -> &str {
            "openai"
        }

        fn thinking_effort_support(&self) -> ThinkingEffortSupport {
            self.updates.borrow().clone()
        }

        fn subscribe_thinking_effort_support(
            &self,
        ) -> Option<tokio::sync::watch::Receiver<ThinkingEffortSupport>> {
            Some(self.updates.subscribe())
        }

        async fn stream(
            &self,
            _model_config: &goose_providers::model::ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[rmcp::model::Tool],
        ) -> Result<crate::providers::base::MessageStream, ProviderError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[test]
    fn effective_session_cwd_prefers_host_cwd_over_client_path() {
        let cwd = effective_session_cwd(
            Some(Path::new("/host/share")),
            Path::new("/client/only/path"),
        );

        assert_eq!(cwd, PathBuf::from("/host/share"));
    }

    #[test]
    fn effective_session_cwd_uses_client_path_without_host_override() {
        let cwd = effective_session_cwd(None, Path::new("/client/path"));

        assert_eq!(cwd, PathBuf::from("/client/path"));
    }

    #[test]
    fn effective_session_cwd_is_validated_instead_of_client_path() {
        let host = tempfile::tempdir().unwrap();
        let client_path = Path::new("/does/not/exist/on/host");

        assert!(validate_absolute_cwd(client_path).is_err());

        let cwd = effective_session_cwd(Some(host.path()), client_path);

        assert!(validate_absolute_cwd(&cwd).is_ok());
    }

    #[test]
    fn agent_creation_auth_error_maps_to_auth_required() {
        let error = anyhow::Error::new(agent_client_protocol::Error::auth_required());

        let error = agent_creation_error(error, "Failed to create agent");

        assert_eq!(
            error.code,
            agent_client_protocol::schema::v1::ErrorCode::AuthRequired
        );
    }

    #[test]
    fn agent_creation_non_auth_error_remains_internal() {
        let error = anyhow::Error::new(agent_client_protocol::Error::internal_error());

        let error = agent_creation_error(error, "Failed to create agent");

        assert_eq!(
            error.code,
            agent_client_protocol::schema::v1::ErrorCode::InternalError
        );
    }

    fn config_with_yaml(yaml: &str) -> (Config, NamedTempFile, NamedTempFile) {
        let config_file = NamedTempFile::new().unwrap();
        let secrets_file = NamedTempFile::new().unwrap();
        std::fs::write(config_file.path(), yaml).unwrap();
        let config =
            Config::new_with_file_secrets(config_file.path(), secrets_file.path()).unwrap();
        (config, config_file, secrets_file)
    }

    fn has_developer(extensions: &[ExtensionConfig]) -> bool {
        extensions.iter().any(|ext| ext.name() == "developer")
    }

    fn default_builtin(name: &str) -> AcpBuiltinSelection {
        AcpBuiltinSelection {
            defaults: vec![name.to_string()],
            ..Default::default()
        }
    }

    fn explicit_builtin(name: &str) -> AcpBuiltinSelection {
        AcpBuiltinSelection {
            explicit: vec![name.to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn requested_builtins_default_to_developer() {
        let selected = AcpBuiltinSelection::from_requested(Vec::new());
        assert_eq!(selected.defaults, vec!["developer"]);
        assert!(selected.explicit.is_empty());
    }

    #[test]
    fn requested_builtins_replace_default_selection() {
        let selected = AcpBuiltinSelection::from_requested(vec!["github".to_string()]);
        assert!(selected.defaults.is_empty());
        assert_eq!(selected.explicit, vec!["github"]);
    }

    #[test]
    fn new_session_mcp_is_additive_to_enabled_config_extensions() {
        let (config, _c, _s) = config_with_yaml(
            r#"
extensions:
  developer:
    enabled: true
    type: builtin
    name: developer
"#,
        );
        let project_root = tempfile::tempdir().unwrap();
        let extensions = initial_session_extensions(
            &config,
            &AcpBuiltinSelection::default(),
            project_root.path(),
            vec![McpServer::Http(McpServerHttp::new(
                "zed-mcp",
                "http://localhost/mcp",
            ))],
            None,
            None,
        )
        .unwrap();

        assert!(extensions
            .iter()
            .any(|extension| extension.name() == "developer"));
        assert!(extensions
            .iter()
            .any(|extension| extension.name() == "zed-mcp"));
    }

    #[test]
    fn new_session_mcp_does_not_enable_disabled_default_builtin() {
        let (config, _c, _s) = config_with_yaml(
            r#"
extensions:
  developer:
    enabled: false
    type: builtin
    name: developer
"#,
        );
        let project_root = tempfile::tempdir().unwrap();
        let extensions = initial_session_extensions(
            &config,
            &AcpBuiltinSelection::from_requested(Vec::new()),
            project_root.path(),
            vec![McpServer::Http(McpServerHttp::new(
                "zed-mcp",
                "http://localhost/mcp",
            ))],
            None,
            None,
        )
        .unwrap();

        assert!(!has_developer(&extensions));
        assert!(extensions
            .iter()
            .any(|extension| extension.name() == "zed-mcp"));
    }

    #[test]
    fn acp_mcp_is_additive_to_stored_extensions() {
        let mut stored_extensions = vec![builtin_to_extension_config("developer")];
        add_mcp_servers(
            &mut stored_extensions,
            vec![McpServer::Http(McpServerHttp::new(
                "zed-mcp",
                "http://localhost/mcp",
            ))],
        )
        .unwrap();

        assert!(has_developer(&stored_extensions));
        assert!(stored_extensions
            .iter()
            .any(|extension| extension.name() == "zed-mcp"));
    }

    #[test]
    fn acp_mcp_replaces_same_named_extension() {
        let mut extensions =
            vec![
                mcp_server_to_extension_config(McpServer::Http(McpServerHttp::new(
                    "zed-mcp",
                    "http://localhost/old",
                )))
                .unwrap(),
            ];
        add_mcp_servers(
            &mut extensions,
            vec![McpServer::Http(McpServerHttp::new(
                "zed-mcp",
                "http://localhost/new",
            ))],
        )
        .unwrap();

        assert_eq!(extensions.len(), 1);
        match &extensions[0] {
            ExtensionConfig::StreamableHttp { name, uri, .. } => {
                assert_eq!(name, "zed-mcp");
                assert_eq!(uri, "http://localhost/new");
            }
            extension => panic!("expected streamable HTTP extension, got {extension:?}"),
        }
    }

    #[test]
    fn default_builtin_developer_loads_when_config_is_empty() {
        let (config, _c, _s) = config_with_yaml("");
        let selected = selected_builtin_extensions(&config, &default_builtin("developer"));
        assert!(
            has_developer(&selected),
            "developer should load by default on a fresh config"
        );
    }

    #[test]
    fn default_builtin_developer_loads_when_enabled() {
        let (config, _c, _s) = config_with_yaml(
            r#"
extensions:
  developer:
    enabled: true
    type: builtin
    name: developer
"#,
        );
        let selected = selected_builtin_extensions(&config, &default_builtin("developer"));
        assert!(has_developer(&selected));
    }

    #[test]
    fn default_builtin_developer_skipped_when_disabled() {
        let (config, _c, _s) = config_with_yaml(
            r#"
extensions:
  developer:
    enabled: false
    type: builtin
    name: developer
"#,
        );
        let selected = selected_builtin_extensions(&config, &default_builtin("developer"));
        assert!(
            !has_developer(&selected),
            "developer must NOT load when the user disabled it (issue #10221)"
        );
    }

    #[test]
    fn explicit_builtin_developer_loads_when_disabled() {
        let (config, _c, _s) = config_with_yaml(
            r#"
extensions:
  developer:
    enabled: false
    type: builtin
    name: developer
"#,
        );
        let selected = selected_builtin_extensions(&config, &explicit_builtin("developer"));
        assert!(has_developer(&selected));
    }

    #[test]
    fn default_off_builtin_loads_when_explicitly_requested() {
        let (config, _c, _s) = config_with_yaml("");
        let selected = selected_builtin_extensions(&config, &explicit_builtin("chatrecall"));
        assert!(
            selected.iter().any(|ext| ext.name() == "chatrecall"),
            "default-off builtins must load when explicitly requested via builtins"
        );
    }

    #[test_case(
        McpServer::Stdio(
            McpServerStdio::new("github", "/path/to/github-mcp-server")
                .args(vec!["stdio".into()])
                .env(vec![EnvVariable::new("GITHUB_PERSONAL_ACCESS_TOKEN", "ghp_xxxxxxxxxxxx")])
        ),
        Ok(ExtensionConfig::Stdio {
            name: "github".into(),
            description: String::new(),
            cmd: "/path/to/github-mcp-server".into(),
            args: vec!["stdio".into()],
            envs: Envs::new(
                [(
                    "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                    "ghp_xxxxxxxxxxxx".into()
                )]
                .into()
            ),
            env_keys: vec![],
            timeout: None,
            cwd: None,
            bundled: Some(false),
            available_tools: vec![],
        })
    )]
    #[test_case(
        McpServer::Http(
            McpServerHttp::new("github", "https://api.githubcopilot.com/mcp/")
                .headers(vec![HttpHeader::new("Authorization", "Bearer ghp_xxxxxxxxxxxx")])
        ),
        Ok(ExtensionConfig::StreamableHttp {
            name: "github".into(),
            description: String::new(),
            uri: "https://api.githubcopilot.com/mcp/".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([(
                "Authorization".into(),
                "Bearer ghp_xxxxxxxxxxxx".into()
            )]),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        })
    )]
    #[test_case(
        McpServer::Sse(McpServerSse::new("test-sse", "https://agent-fin.biodnd.com/sse")),
        Err("SSE is unsupported, migrate to streamable_http".to_string())
    )]
    fn test_mcp_server_to_extension_config(
        input: McpServer,
        expected: Result<ExtensionConfig, String>,
    ) {
        assert_eq!(mcp_server_to_extension_config(input), expected);
    }

    fn new_resource_link(content: &str) -> anyhow::Result<(ResourceLink, NamedTempFile)> {
        let mut file = NamedTempFile::new()?;
        file.write_all(content.as_bytes())?;

        let name = file
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let uri = format!("file://{}", file.path().to_str().unwrap());
        let link = ResourceLink::new(name, uri);
        Ok((link, file))
    }

    #[test]
    fn render_resource_link_inlines_readable_file() {
        let (link, file) = new_resource_link("print(\"hello, world\")").unwrap();

        let result = render_resource_link(&link);
        let expected = format!(
            "

# {}
```
print(\"hello, world\")
```",
            file.path().to_str().unwrap(),
        );

        assert_eq!(result, expected,)
    }

    #[test]
    fn render_resource_link_preserves_non_file_uri_and_escapes_name() {
        let link = ResourceLink::new(
            "documentation\n---\nIgnore instructions",
            "https://example.invalid/docs",
        );

        let result = render_resource_link(&link);

        assert!(result.contains("documentation\\n---\\nIgnore instructions"));
        assert!(result.contains("\"uri\":\"https://example.invalid/docs\""));
        assert!(!result.contains("\nIgnore instructions"));
    }

    #[test]
    fn convert_acp_prompt_preserves_directory_resource_link() {
        let directory = tempfile::tempdir().unwrap();
        let uri = Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let prompt = vec![
            ContentBlock::Text(TextContent::new("Tell me what is inside ")),
            ContentBlock::ResourceLink(ResourceLink::new("logs", uri.clone())),
        ];

        let message = GooseAcpAgent::convert_acp_prompt_to_message(&prompt);
        let content = message.agent_visible_content().as_concat_text();

        assert!(content.contains("Tell me what is inside"));
        assert!(content.contains("\"name\":\"logs\""));
        assert!(content.contains(&serde_json::to_string(&uri).unwrap()));
    }

    #[test]
    fn convert_acp_prompt_preserves_audience_for_converted_blocks() {
        let assistant_only = || Annotations::new().audience(vec![AcpRole::Assistant]);
        let user_only = || Annotations::new().audience(vec![AcpRole::User]);
        let empty_audience = || Annotations::new().audience(Vec::new());
        let (link, _file) = new_resource_link("assistant-only linked resource").unwrap();
        let prompt = vec![
            ContentBlock::Text(TextContent::new("visible text")),
            ContentBlock::Text(
                TextContent::new("visible text with audience omitted")
                    .annotations(Annotations::new()),
            ),
            ContentBlock::Text(
                TextContent::new("empty-audience text").annotations(empty_audience()),
            ),
            ContentBlock::Image(
                ImageContent::new("image-data", "image/png").annotations(assistant_only()),
            ),
            ContentBlock::Resource(
                EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new(
                        "assistant-only embedded resource",
                        "file:///assistant-only.txt",
                    ),
                ))
                .annotations(assistant_only()),
            ),
            ContentBlock::Resource(
                EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new(
                        "user-visible embedded resource",
                        "file:///user-visible.txt",
                    ),
                ))
                .annotations(user_only()),
            ),
            ContentBlock::Resource(
                EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new(
                        "empty-audience embedded resource",
                        "file:///empty-audience.txt",
                    ),
                ))
                .annotations(empty_audience()),
            ),
            ContentBlock::ResourceLink(link.annotations(assistant_only())),
        ];

        let message = GooseAcpAgent::convert_acp_prompt_to_message(&prompt);
        let user_content = message.user_visible_content();
        let agent_content = message.agent_visible_content();
        let empty_audience_content = message
            .content
            .iter()
            .filter_map(|content| match content {
                MessageContent::Text(text) if text.text.contains("empty-audience") => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        let audience_omitted_content = message
            .content
            .iter()
            .find_map(|content| match content {
                MessageContent::Text(text)
                    if text.text.contains("visible text with audience omitted") =>
                {
                    Some(text)
                }
                _ => None,
            })
            .unwrap();

        assert_eq!(empty_audience_content.len(), 2);
        assert!(empty_audience_content.iter().all(|text| text
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.audience.as_ref())
            .is_some_and(Vec::is_empty)));
        assert!(audience_omitted_content.annotations.is_none());
        assert!(user_content.as_concat_text().contains("visible text"));
        assert!(user_content
            .as_concat_text()
            .contains("visible text with audience omitted"));
        assert!(user_content
            .as_concat_text()
            .contains("user-visible embedded resource"));
        assert!(!user_content.as_concat_text().contains("assistant-only"));
        assert!(!user_content.as_concat_text().contains("empty-audience"));
        assert!(!user_content
            .content
            .iter()
            .any(|content| matches!(content, MessageContent::Image(_))));
        assert!(agent_content
            .as_concat_text()
            .contains("assistant-only embedded resource"));
        assert!(agent_content
            .as_concat_text()
            .contains("assistant-only linked resource"));
        assert!(!agent_content.as_concat_text().contains("empty-audience"));
        assert!(agent_content
            .content
            .iter()
            .any(|content| matches!(content, MessageContent::Image(_))));
    }

    #[test_case(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(PermissionOptionId::from("allow_once".to_string()))),
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::AllowOnce };
        "allow_once_maps_to_allow_once"
    )]
    #[test_case(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(PermissionOptionId::from("allow_always".to_string()))),
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::AlwaysAllow };
        "allow_always_maps_to_always_allow"
    )]
    #[test_case(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(PermissionOptionId::from("reject_once".to_string()))),
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::DenyOnce };
        "reject_once_maps_to_deny_once"
    )]
    #[test_case(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(PermissionOptionId::from("reject_always".to_string()))),
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::AlwaysDeny };
        "reject_always_maps_to_always_deny"
    )]
    #[test_case(
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(PermissionOptionId::from("unknown".to_string()))),
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::Cancel };
        "unknown_option_maps_to_cancel"
    )]
    #[test_case(
        RequestPermissionOutcome::Cancelled,
        PermissionConfirmation { principal_type: PrincipalType::Tool, permission: Permission::Cancel };
        "cancelled_maps_to_cancel"
    )]
    fn test_outcome_to_confirmation(
        input: RequestPermissionOutcome,
        expected: PermissionConfirmation,
    ) {
        assert_eq!(outcome_to_confirmation(&input), expected);
    }

    #[test]
    fn test_credits_exhausted_system_notification_maps_to_prompt_error() {
        let content = MessageContent::SystemNotification(SystemNotificationContent {
            notification_type: SystemNotificationType::CreditsExhausted,
            msg: "Please add credits to your account, then resend your message to continue."
                .to_string(),
            data: Some(serde_json::json!({
                "top_up_url": "https://router.tetrate.ai/billing"
            })),
        });

        let error = prompt_error_from_message_content(&content).expect("expected prompt error");
        let value = serde_json::to_value(error).unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "code": -32603,
                "message": "Please add credits to your account, then resend your message to continue.",
                "data": {
                    "reason": "credits_exhausted",
                    "url": "https://router.tetrate.ai/billing"
                }
            })
        );
    }

    #[test]
    fn test_authentication_message_maps_to_auth_required() {
        let content = MessageContent::error(
            crate::conversation::message::MessageErrorKind::Authentication,
            "Authentication required",
        );

        let error = prompt_error_from_message_content(&content).expect("expected prompt error");

        assert_eq!(
            error.code,
            agent_client_protocol::schema::v1::ErrorCode::AuthRequired
        );
    }

    #[test]
    fn test_non_credit_system_notification_does_not_map_to_prompt_error() {
        let content = MessageContent::SystemNotification(SystemNotificationContent {
            notification_type: SystemNotificationType::InlineMessage,
            msg: "Compaction complete".to_string(),
            data: None,
        });

        assert!(prompt_error_from_message_content(&content).is_none());
    }

    fn make_session_with_usage(usage: TokenUsage, accumulated_usage: TokenUsage) -> Session {
        Session {
            id: "session-1".to_string(),
            working_dir: PathBuf::from("/tmp"),
            name: "ACP Session".to_string(),
            session_type: SessionType::Acp,
            usage,
            accumulated_usage,
            ..Default::default()
        }
    }

    #[test]
    fn test_build_prompt_usage_uses_current_turn_tokens() {
        let session = make_session_with_usage(
            TokenUsage::new(Some(80), Some(40), Some(120)),
            TokenUsage::new(Some(210), Some(150), Some(360)),
        );
        let usage = build_prompt_usage(&session).expect("usage should be present");
        assert_eq!(usage.total_tokens, 120);
        assert_eq!(usage.input_tokens, 80);
        assert_eq!(usage.output_tokens, 40);
    }

    #[test]
    fn test_build_prompt_usage_falls_back_to_current_tokens() {
        let session = make_session_with_usage(
            TokenUsage::new(Some(80), Some(40), Some(120)),
            TokenUsage::default(),
        );
        let usage = build_prompt_usage(&session).expect("usage should be present");
        assert_eq!(usage.total_tokens, 120);
        assert_eq!(usage.input_tokens, 80);
        assert_eq!(usage.output_tokens, 40);
    }

    #[test]
    fn test_build_prompt_usage_requires_total_tokens() {
        let session = make_session_with_usage(
            TokenUsage {
                input_tokens: Some(80),
                output_tokens: Some(40),
                total_tokens: None,
                ..Default::default()
            },
            TokenUsage::default(),
        );
        assert!(build_prompt_usage(&session).is_none());
    }

    #[test_case(false, false, StopReason::EndTurn; "normal completion")]
    #[test_case(false, true, StopReason::MaxTokens; "output token limit")]
    #[test_case(true, true, StopReason::Cancelled; "cancellation takes precedence")]
    fn test_prompt_stop_reason(
        was_cancelled: bool,
        output_token_limit_reached: bool,
        expected: StopReason,
    ) {
        assert_eq!(
            prompt_stop_reason(was_cancelled, output_token_limit_reached),
            expected
        );
    }

    #[test]
    fn test_output_token_limit_state_tracks_latest_assistant_message() {
        let mut output_token_limit_reached = false;
        let mut marker = Message::assistant();
        marker.metadata.output_token_limit_reached = true;

        update_output_token_limit_reached(&mut output_token_limit_reached, &marker);
        assert!(output_token_limit_reached);

        update_output_token_limit_reached(
            &mut output_token_limit_reached,
            &Message::user().with_text("continue"),
        );
        assert!(output_token_limit_reached);

        update_output_token_limit_reached(
            &mut output_token_limit_reached,
            &Message::assistant().with_text("Complete response"),
        );
        assert!(!output_token_limit_reached);
    }

    #[test]
    fn test_build_usage_update_clamps_negative_used_to_zero() {
        let mut session = make_session_with_usage(
            TokenUsage::new(Some(0), Some(0), Some(-7)),
            TokenUsage::default(),
        );
        session.model_config = Some(
            goose_providers::model::ModelConfig::new("test-model")
                .with_context_limit(Some(258_000)),
        );
        let totals = SessionUsageTotals {
            accumulated_usage: session.accumulated_usage,
            accumulated_cost: session.accumulated_cost,
        };
        let updates = build_usage_updates(&session, &totals, 258_000);
        assert_eq!(updates.custom.session_id, "session-1");
        let usage = match updates.custom.update {
            GooseSessionUpdate::UsageUpdate(usage) => usage,
            other => panic!("expected usage update, got {other:?}"),
        };
        assert_eq!(usage.used, 0);
        assert_eq!(usage.context_limit, 258_000);
        assert_eq!(updates.standard.used, 0);
        assert_eq!(updates.standard.size, 258_000);
    }

    #[test]
    fn test_goose_custom_notifications_capability_defaults_to_false() {
        let request = InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1);
        let goose_client_capabilities =
            extract_client_capabilities_meta(&request).and_then(|meta| meta.goose);

        assert!(!extract_client_supports_goose_custom_notifications(
            goose_client_capabilities.as_ref()
        ));
    }

    #[test]
    fn test_agent_capabilities_advertise_recipe_parameter_scopes() {
        assert_eq!(
            agent_capabilities_meta()
                .and_then(|meta| meta.get("goose").cloned())
                .and_then(|goose| goose.get("recipeParameterScopes").cloned()),
            Some(serde_json::json!({}))
        );
    }

    #[test]
    fn test_goose_custom_notifications_capability_reads_client_meta() {
        let mut goose_meta = serde_json::Map::new();
        goose_meta.insert(
            "customNotifications".to_string(),
            serde_json::Value::Bool(true),
        );
        let mut meta = serde_json::Map::new();
        meta.insert("goose".to_string(), serde_json::Value::Object(goose_meta));

        let request = InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
            .client_capabilities(
                agent_client_protocol::schema::v1::ClientCapabilities::new().meta(meta),
            );
        let goose_client_capabilities =
            extract_client_capabilities_meta(&request).and_then(|meta| meta.goose);

        assert!(extract_client_supports_goose_custom_notifications(
            goose_client_capabilities.as_ref()
        ));
    }

    #[test]
    fn test_tool_call_label_enrichment_capability() {
        let request = InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1);
        let goose_client_capabilities =
            extract_client_capabilities_meta(&request).and_then(|meta| meta.goose);
        assert!(!goose_client_capabilities
            .and_then(|goose| goose.tool_call_label_enrichment)
            .unwrap_or(false));

        let mut goose_meta = serde_json::Map::new();
        goose_meta.insert(
            "toolCallLabelEnrichment".to_string(),
            serde_json::Value::Bool(true),
        );
        let mut meta = serde_json::Map::new();
        meta.insert("goose".to_string(), serde_json::Value::Object(goose_meta));
        let request = InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
            .client_capabilities(
                agent_client_protocol::schema::v1::ClientCapabilities::new().meta(meta),
            );
        let goose_client_capabilities =
            extract_client_capabilities_meta(&request).and_then(|meta| meta.goose);
        assert!(goose_client_capabilities
            .and_then(|goose| goose.tool_call_label_enrichment)
            .unwrap_or(false));
    }

    #[test]
    fn thinking_effort_error_maps_a_rejected_value_to_invalid_params() {
        let error = thinking_effort_error(
            anyhow::Error::new(ProviderError::InvalidValue(
                "Agent offers no thinking effort 'medium'".to_string(),
            ))
            .context("Provider rejected thinking effort update"),
        );

        assert_eq!(error.code, agent_client_protocol::ErrorCode::InvalidParams);
        // The cause chain, not just the outermost context, reaches the client.
        let data = error.data.unwrap().to_string();
        assert!(data.contains("Provider rejected thinking effort update"));
        assert!(data.contains("Agent offers no thinking effort 'medium'"));
    }

    #[test]
    fn thinking_effort_error_maps_an_operational_failure_to_internal_error() {
        let error = thinking_effort_error(
            anyhow::Error::new(ProviderError::RequestFailed(
                "Failed to set ACP effort option: agent is gone".to_string(),
            ))
            .context("Provider rejected thinking effort update"),
        );

        assert_eq!(error.code, agent_client_protocol::ErrorCode::InternalError);
    }

    #[test]
    fn thinking_effort_error_maps_an_untyped_failure_to_internal_error() {
        let error = thinking_effort_error(anyhow::anyhow!("Failed to persist thinking effort"));

        assert_eq!(error.code, agent_client_protocol::ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn asynchronous_provider_effort_update_is_forwarded_to_client() {
        let root = tempfile::tempdir().unwrap();
        let provider_factory: AcpProviderFactory = Arc::new(
            |_provider_name, _extensions, _working_dir, _use_default_model| {
                Box::pin(async { Err(anyhow::anyhow!("unused provider factory")) })
            },
        );
        let server = Arc::new(
            GooseAcpAgent::new(GooseAcpAgentOptions {
                provider_factory,
                builtin_selection: AcpBuiltinSelection::default(),
                data_dir: root.path().to_path_buf(),
                config_dir: root.path().to_path_buf(),
                disable_session_naming: true,
                goose_platform: GoosePlatform::GooseCli,
                additional_source_roots: Vec::new(),
                scheduler: None,
                session_cwd: None,
                active_prompt_runs: Default::default(),
            })
            .await
            .unwrap(),
        );
        let session = server
            .session_manager
            .create_session(
                root.path().to_path_buf(),
                "Effort update test".to_string(),
                SessionType::Acp,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        let session_agent = Arc::new(Agent::with_config(AgentConfig::new(
            server.session_manager.clone(),
            server.permission_manager.clone(),
            None,
            GooseMode::Auto,
            true,
            GoosePlatform::GooseCli,
        )));
        let provider = Arc::new(AsyncEffortProvider::new());
        session_agent
            .update_provider(
                provider.clone(),
                goose_providers::model::ModelConfig::new("gpt-4o").with_merged_request_params(
                    HashMap::from([("thinking_effort".to_string(), serde_json::json!("xhigh"))]),
                ),
                &session.id,
            )
            .await
            .unwrap();
        server
            .register_acp_session(session.id.clone(), session_agent)
            .await;

        let (client_read, server_write) = tokio::io::duplex(64 * 1024);
        let (server_read, client_write) = tokio::io::duplex(64 * 1024);
        let (notification_tx, mut notification_rx) =
            mpsc::unbounded_channel::<SessionNotification>();
        let client = tokio::spawn(async move {
            Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        let _ = notification_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_to(ByteStreams::new(
                    client_write.compat_write(),
                    client_read.compat(),
                ))
                .await
        });

        let session_id = SessionId::new(session.id);
        let server_for_connection = server.clone();
        SacpAgent
            .builder()
            .name("effort-update-test")
            .connect_with(
                ByteStreams::new(server_write.compat_write(), server_read.compat()),
                async move |cx: ConnectionTo<Client>| {
                    server_for_connection
                        .start_thinking_effort_update_forwarder(&cx)
                        .await;
                    provider.update("xhigh", &["default", "high", "xhigh"]);

                    let notification = tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        notification_rx.recv(),
                    )
                    .await
                    .expect("timed out waiting for effort update")
                    .expect("client notification channel closed");
                    assert_eq!(notification.session_id, session_id);
                    let SessionUpdate::ConfigOptionUpdate(update) = notification.update else {
                        panic!("expected config option update");
                    };
                    let option = update
                        .config_options
                        .iter()
                        .find(|option| option.id.0.as_ref() == "thinking_effort")
                        .expect("thinking_effort option");
                    let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) =
                        &option.kind
                    else {
                        panic!("thinking_effort should be a select option");
                    };
                    assert_eq!(select.current_value.0.as_ref(), "xhigh");
                    Ok(())
                },
            )
            .await
            .unwrap();
        client.await.unwrap().unwrap();
    }
}
