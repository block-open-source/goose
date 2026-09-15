use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::stream::{self, FuturesUnordered, StreamExt};
use futures::Stream;
use futures::{future, FutureExt};
use once_cell::sync::Lazy;
use rmcp::service::ServiceError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use super::container::Container;
use super::extension::{
    ExtensionConfig, ExtensionError, ExtensionInfo, ExtensionResult, PlatformExtensionContext,
    PLATFORM_EXTENSIONS,
};
use super::tool_execution::{ToolCallContext, ToolCallNotificationEmitter, ToolCallResult};
use super::types::SharedProvider;
use crate::action_required_manager::ActionRequiredManager;
use crate::agents::extension::Envs;
use crate::agents::mcp_client::{
    ConnectContext, GooseMcpClientCapabilities, GooseMcpHostInfo, McpClientTrait,
};
use crate::agents::reply_parts::is_tool_visible_to_app;
use crate::config::extensions::name_to_key;
use crate::config::{get_all_extensions, Config};
use crate::oauth::GooseCredentialStore;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, ErrorData, GetPromptResult,
    ListResourcesResult, ListToolsResult, MetaObject, Prompt, Resource, ResourceContents,
    ServerInfo, ServerNotification, Tool,
};
use schemars::_private::NoSerialize;
use serde_json::Value;

mod builtin;
mod stdio;
mod streamable_http;

type McpClientBox = Arc<dyn McpClientTrait>;

const TOOL_CALL_NOTIFICATION_CHANNEL_CAPACITY: usize = 32;

struct ActionRequiredStream {
    inner: ReceiverStream<crate::conversation::message::Message>,
    manager: Arc<ActionRequiredManager>,
    session_id: String,
    tool_call_request_id: String,
}

impl ActionRequiredStream {
    fn new(
        receiver: tokio::sync::mpsc::Receiver<crate::conversation::message::Message>,
        manager: Arc<ActionRequiredManager>,
        session_id: String,
        tool_call_request_id: String,
    ) -> Self {
        Self {
            inner: ReceiverStream::new(receiver),
            manager,
            session_id,
            tool_call_request_id,
        }
    }
}

impl Stream for ActionRequiredStream {
    type Item = crate::conversation::message::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for ActionRequiredStream {
    fn drop(&mut self) {
        let manager = self.manager.clone();
        let session_id = self.session_id.clone();
        let tool_call_request_id = self.tool_call_request_id.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            manager
                .unregister_action_required_stream(&session_id, &tool_call_request_id)
                .await;
        });
    }
}

static RE_ENV_BRACES: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\$\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}").expect("valid regex"));

static RE_ENV_SIMPLE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex"));

fn resolve_timeout(timeout: Option<u64>) -> u64 {
    timeout.unwrap_or_else(|| {
        Config::global()
            .get_goose_default_extension_timeout()
            .unwrap_or(crate::config::DEFAULT_EXTENSION_TIMEOUT)
    })
}

struct Extension {
    pub config: ExtensionConfig,
    /// Resolved config snapshot (with secrets from keyring substituted)
    /// captured at client-creation time. Used to detect secret rotation
    /// without re-reading the keyring on every comparison. Only held in
    /// memory — never serialized to disk.
    resolved_config: ExtensionConfig,

    client: McpClientBox,
    server_info: Option<ServerInfo>,
}

impl Extension {
    fn new(
        config: ExtensionConfig,
        resolved_config: ExtensionConfig,
        client: McpClientBox,
        server_info: Option<ServerInfo>,
    ) -> Self {
        Self {
            client,
            config,
            resolved_config,
            server_info,
        }
    }

    fn supports_resources(&self) -> bool {
        self.server_info
            .as_ref()
            .and_then(|info| info.capabilities.resources.as_ref())
            .is_some()
    }

    fn get_instructions(&self) -> Option<String> {
        self.client.get_instructions()
    }

    fn get_client(&self) -> McpClientBox {
        self.client.clone()
    }
}

pub struct ExtensionManagerCapabilities {
    pub mcpui: bool,
    pub host_info: Option<GooseMcpHostInfo>,
    pub elicitation_handler: Option<crate::agents::mcp_client::ElicitationHandler>,
    pub protocol_version: Option<rmcp::model::ProtocolVersion>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GooseMcpAppToolAttachment {
    pub tool_name: String,
    pub tool_name_is_actual: bool,
    pub extension_name: String,
    pub resource_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_meta: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
}

pub(crate) const TRUSTED_TOOL_UPDATE_META_KEY: &str = "__goose_tool_update_meta";

/// Manages goose extensions / MCP clients and their interactions
pub struct ExtensionManager {
    extensions: Mutex<HashMap<String, Extension>>,
    context: PlatformExtensionContext,
    provider: SharedProvider,
    tools_cache: Mutex<Option<Arc<Vec<Tool>>>>,
    tools_cache_version: AtomicU64,
    client_name: String,
    capabilities: ExtensionManagerCapabilities,
}

/// A flattened representation of a resource used by the agent to prepare inference
#[derive(Debug, Clone)]
pub struct ResourceItem {
    pub extension_name: String, // The name of the extension that owns the resource
    pub uri: String,            // The URI of the resource
    pub name: String,           // The name of the resource
    pub content: String,        // The content of the resource
    pub timestamp: DateTime<Utc>, // The timestamp of the resource
    pub priority: f32,          // The priority of the resource
    pub token_count: Option<u32>, // The token count of the resource (filled in by the agent)
}

impl ResourceItem {
    pub fn new(
        extension_name: String,
        uri: String,
        name: String,
        content: String,
        timestamp: DateTime<Utc>,
        priority: f32,
    ) -> Self {
        Self {
            extension_name,
            uri,
            name,
            content,
            timestamp,
            priority,
            token_count: None,
        }
    }
}

fn require_str_parameter<'a>(v: &'a serde_json::Value, name: &str) -> Result<&'a str, ErrorData> {
    let v = v.get(name).ok_or_else(|| {
        ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            format!("The parameter {name} is required"),
            None,
        )
    })?;
    match v.as_str() {
        Some(r) => Ok(r),
        None => Err(ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            format!("The parameter {name} must be a string"),
            None,
        )),
    }
}

pub fn get_parameter_names(tool: &Tool) -> Vec<String> {
    let mut names: Vec<String> = tool
        .input_schema
        .get("properties")
        .and_then(|props| props.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

const TOOL_EXTENSION_META_KEY: &str = "goose_extension";

pub fn get_tool_owner(tool: &Tool) -> Option<String> {
    tool.meta
        .as_ref()
        .and_then(|m| m.0.get(TOOL_EXTENSION_META_KEY))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn is_tool_owned_by_extension(tool: &Tool, extension_name: &str) -> bool {
    let expected_owner = name_to_key(extension_name);
    get_tool_owner(tool).is_some_and(|owner| name_to_key(&owner) == expected_owner)
}

/// `tools` pairs each advertised public tool name with its owning extension's
/// key, when known (`None` for tools with no owner metadata, e.g. those
/// appended outside the extension manager).
pub(crate) fn recover_mangled_tool_name<'a>(
    emitted: &str,
    tools: impl Iterator<Item = (&'a str, Option<&'a str>)>,
) -> Option<String> {
    let trimmed = emitted.trim();
    let stripped = trimmed
        .strip_prefix("functions.")
        .or_else(|| trimmed.strip_prefix("functions:"))
        .unwrap_or(trimmed);

    let mut matched: Option<&str> = None;
    for (name, owner) in tools {
        // Prefixed tools: the model turns Goose's "__" separator into a dot
        // ("developer__shell" -> "developer.shell").
        let separator_mangled = name
            .split_once("__")
            .map(|(extension, tool)| format!("{extension}.{tool}"));

        // Unprefixed tools (e.g. platform extensions like "developer" with
        // unprefixed_tools=true) carry no "__" in their public name at all —
        // the owner is only in metadata — so the model's "developer.shell"
        // has to be checked against "{owner}.{name}" instead (see #9486).
        let owner_mangled = owner.map(|o| format!("{o}.{name}"));
        let owner_prefixed = owner.map(|o| format!("{o}__{name}"));

        let matches = stripped == name
            || separator_mangled.as_deref() == Some(stripped)
            || owner_mangled.as_deref() == Some(stripped)
            || owner_prefixed.as_deref() == Some(stripped);
        if name == emitted || !matches {
            continue;
        }

        match matched {
            None => matched = Some(name),
            Some(prev) if prev == name => {}
            Some(_) => return None,
        }
    }
    matched.map(|s| s.to_string())
}

fn get_tool_meta_value(tool: &Tool) -> Option<Value> {
    tool.meta.as_ref().map(|meta| Value::Object(meta.0.clone()))
}

pub(crate) fn get_tool_resource_uri(tool: &Tool) -> Option<String> {
    tool.meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(Value::as_object)
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn remove_untrusted_mcp_app_meta(result: &mut CallToolResult) {
    let Some(meta) = result.meta.as_mut() else {
        return;
    };

    meta.0.remove(TRUSTED_TOOL_UPDATE_META_KEY);

    let remove_goose = meta
        .0
        .get_mut("goose")
        .and_then(Value::as_object_mut)
        .map(|goose_meta| {
            goose_meta.remove("mcpApp");
            goose_meta.is_empty()
        })
        .unwrap_or(false);

    if remove_goose {
        meta.0.remove("goose");
    }

    if meta.0.is_empty() {
        result.meta = None;
    }
}

fn insert_trusted_tool_update_meta(
    result: &mut CallToolResult,
    attachment: &GooseMcpAppToolAttachment,
) {
    let Ok(attachment_value) = serde_json::to_value(attachment) else {
        return;
    };

    let mut meta_map = result
        .meta
        .as_ref()
        .map(|meta| meta.0.clone())
        .unwrap_or_default();
    let mut trusted_meta = serde_json::Map::new();
    trusted_meta.insert("mcpApp".to_string(), attachment_value);
    meta_map.insert(
        TRUSTED_TOOL_UPDATE_META_KEY.to_string(),
        Value::Object(trusted_meta),
    );
    result.meta = Some(MetaObject(meta_map));
}

fn is_unprefixed_extension(config: &ExtensionConfig) -> bool {
    match config {
        ExtensionConfig::Platform { name, .. } | ExtensionConfig::Builtin { name, .. } => {
            PLATFORM_EXTENSIONS
                .get(name_to_key(name).as_str())
                .is_some_and(|def| def.unprefixed_tools)
        }
        _ => false,
    }
}

/// Returns true if the named extension is a first-class platform extension
/// whose tools are exposed unprefixed and remain visible during code execution mode.
pub fn is_first_class_extension(name: &str) -> bool {
    PLATFORM_EXTENSIONS
        .get(name_to_key(name).as_str())
        .is_some_and(|def| def.unprefixed_tools)
}

pub fn is_hidden_extension(name: &str) -> bool {
    PLATFORM_EXTENSIONS
        .get(name_to_key(name).as_str())
        .is_some_and(|def| def.hidden)
}

/// Result of resolving a tool call to its owning extension
struct ResolvedTool {
    extension_name: String,
    actual_tool_name: String,
    client: McpClientBox,
    tool_meta: Option<Value>,
    resource_uri: Option<String>,
}

/// Merge environment variables from direct envs and keychain-stored env_keys
pub(crate) async fn merge_environments(
    envs: &Envs,
    env_keys: &[String],
    ext_name: &str,
    config: &Config,
) -> Result<HashMap<String, String>, ExtensionError> {
    let mut all_envs = envs.get_env();

    for key in env_keys {
        if all_envs.contains_key(key) {
            continue;
        }

        match config.get(key, true) {
            Ok(value) => {
                if value.is_null() {
                    warn!(
                        key = %key,
                        ext_name = %ext_name,
                        "Secret key not found in config (returned null)."
                    );
                    continue;
                }

                if let Some(str_val) = value.as_str() {
                    all_envs.insert(key.clone(), str_val.to_string());
                } else {
                    warn!(
                        key = %key,
                        ext_name = %ext_name,
                        value_type = %value.get("type").and_then(|t| t.as_str()).unwrap_or("unknown"),
                        "Secret value is not a string; skipping."
                    );
                }
            }
            Err(e) => {
                error!(
                    key = %key,
                    ext_name = %ext_name,
                    error = %e,
                    "Failed to fetch secret from config."
                );
                return Err(ExtensionError::ConfigError(format!(
                    "Failed to fetch secret '{}' from config: {}",
                    key, e
                )));
            }
        }
    }

    Ok(Envs::new(all_envs).get_env())
}

/// Substitute environment variables in a string. Supports both ${VAR} and $VAR syntax.
pub(crate) fn substitute_env_vars(value: &str, env_map: &HashMap<String, String>) -> String {
    let mut result = value.to_string();

    for cap in RE_ENV_BRACES.captures_iter(value) {
        if let Some(var_name) = cap.get(1) {
            if let Some(env_value) = env_map.get(var_name.as_str()) {
                result = result.replace(&cap[0], env_value);
            }
        }
    }

    // Scan the original input for $VAR patterns (not the post-substitution result)
    // to avoid recursive expansion when a substituted value contains $OTHER_VAR.
    for cap in RE_ENV_SIMPLE.captures_iter(value) {
        if let Some(var_name) = cap.get(1) {
            if !value.contains(&format!("${{{}}}", var_name.as_str())) {
                if let Some(env_value) = env_map.get(var_name.as_str()) {
                    result = result.replace(&cap[0], env_value);
                }
            }
        }
    }

    result
}

impl ExtensionManager {
    fn mcp_client_capabilities(&self) -> GooseMcpClientCapabilities {
        GooseMcpClientCapabilities {
            mcpui: self.capabilities.mcpui,
            host_info: self.capabilities.host_info.clone(),
            elicitation_handler: self.capabilities.elicitation_handler.clone(),
            protocol_version: self.capabilities.protocol_version.clone(),
        }
    }

    pub fn new(
        provider: SharedProvider,
        session_manager: Arc<crate::session::SessionManager>,
        scheduler: Option<Arc<dyn crate::scheduler_trait::SchedulerTrait>>,
        client_name: String,
        capabilities: ExtensionManagerCapabilities,
        use_login_shell_path: bool,
    ) -> Self {
        Self {
            extensions: Mutex::new(HashMap::new()),
            context: PlatformExtensionContext {
                extension_manager: None,
                session_manager,
                scheduler,
                session: None,
                use_login_shell_path,
            },
            provider,
            tools_cache: Mutex::new(None),
            tools_cache_version: AtomicU64::new(0),
            client_name,
            capabilities,
        }
    }

    pub fn new_without_provider(data_dir: std::path::PathBuf) -> Self {
        let session_manager = Arc::new(crate::session::SessionManager::new(data_dir));
        Self::new(
            Arc::new(Mutex::new(None)),
            session_manager,
            None,
            "goose-cli".to_string(),
            ExtensionManagerCapabilities {
                mcpui: false,
                host_info: None,
                elicitation_handler: None,
                protocol_version: None,
            },
            false,
        )
    }

    pub fn get_context(&self) -> &PlatformExtensionContext {
        &self.context
    }

    pub fn get_provider(&self) -> &SharedProvider {
        &self.provider
    }

    pub async fn supports_resources(&self) -> bool {
        self.extensions
            .lock()
            .await
            .values()
            .any(|ext| ext.supports_resources())
    }

    /// Add an extension with an optional working directory.
    /// If working_dir is None, falls back to current_dir.
    pub async fn add_extension(
        self: &Arc<Self>,
        config: ExtensionConfig,
        working_dir: Option<PathBuf>,
        container: Option<&Container>,
        session_id: Option<&str>,
    ) -> ExtensionResult<()> {
        let sanitized_name = config.key();

        // Compare both the unresolved config (to detect structural changes like
        // migrating from plaintext envs to env_keys) and the resolved config (to
        // detect secret rotation where only keyring values changed). Only skip
        // restart if both match.
        let resolved_config = config.clone().resolve(Config::global()).await?;

        if let Some(existing) = self.extensions.lock().await.get(&sanitized_name) {
            if existing.config == config && existing.resolved_config == resolved_config {
                return Ok(());
            }
            tracing::debug!(
                name = sanitized_name,
                "extension config changed, restarting with updated config"
            );
        }

        let working_dir = working_dir
            .or_else(|| std::env::var("GOOSE_WORKING_DIR").ok().map(PathBuf::from))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let ctx = |timeout: Option<u64>, working_dir: PathBuf| ConnectContext {
            timeout: Duration::from_secs(resolve_timeout(timeout)),
            provider: self.provider.clone(),
            client_name: self.client_name.clone(),
            capabilities: self.mcp_client_capabilities(),
            working_dir,
            docker_container: None,
            action_required: self.context.session_manager.action_required(),
            extension_manager: Arc::downgrade(self),
        };

        let client: Box<dyn McpClientTrait> = match &config {
            ExtensionConfig::StreamableHttp {
                uri,
                timeout,
                headers,
                name,
                envs,
                env_keys,
                socket,
                client_id,
                client_secret_key,
                scopes,
                ..
            } => {
                let all_envs =
                    merge_environments(envs, env_keys, &sanitized_name, Config::global()).await?;
                let static_oauth_client = streamable_http::resolve_static_oauth_client(
                    client_id.as_deref(),
                    client_secret_key.as_deref(),
                    scopes,
                    &all_envs,
                    Config::global(),
                )
                .map_err(|error| *error)?;
                let params = streamable_http::ConnectParams {
                    uri: substitute_env_vars(uri, &all_envs),
                    name: name.clone(),
                    headers: headers
                        .iter()
                        .map(|(k, v)| (k.clone(), substitute_env_vars(v, &all_envs)))
                        .collect(),
                    static_oauth_client,
                    ctx: ctx(*timeout, working_dir),
                };
                let socket = socket.as_ref().map(|s| substitute_env_vars(s, &all_envs));
                streamable_http::connect(
                    params,
                    socket.as_deref(),
                    Box::new(GooseCredentialStore::new(name.to_string())),
                )
                .await?
            }
            ExtensionConfig::Builtin { name, .. } | ExtensionConfig::Platform { name, .. }
                if PLATFORM_EXTENSIONS.contains_key(name_to_key(name).as_str()) =>
            {
                let def = &PLATFORM_EXTENSIONS[name_to_key(name).as_str()];
                let mut context = self.context.clone();
                context.extension_manager = Some(Arc::downgrade(self));
                if let Some(id) = session_id {
                    if let Ok(session) = self.context.session_manager.get_session(id, false).await {
                        context.session = Some(Arc::new(session));
                    }
                }
                // A platform extension the host cannot provide (no scheduler
                // service, say) declines rather than registering with no tools.
                let Some(client) = (def.client_factory)(context) else {
                    return Ok(());
                };
                client
            }
            ExtensionConfig::Builtin { name, timeout, .. } => {
                builtin::connect(name, container, ctx(*timeout, working_dir)).await?
            }
            ExtensionConfig::Platform { name, .. } => {
                builtin::connect(name, container, ctx(None, working_dir)).await?
            }
            ExtensionConfig::Stdio {
                cmd,
                args,
                envs,
                env_keys,
                timeout,
                cwd,
                ..
            } => {
                let mut all_envs =
                    merge_environments(envs, env_keys, &sanitized_name, Config::global()).await?;
                if let Some(sid) = session_id {
                    all_envs.insert("AGENT_SESSION_ID".to_string(), sid.to_string());
                }
                let working_dir = cwd.as_deref().map(PathBuf::from).unwrap_or(working_dir);
                Box::new(
                    stdio::connect(cmd, args, all_envs, container, ctx(*timeout, working_dir))
                        .await?,
                )
            }
        };

        let server_info = client.get_info().cloned();

        let mut extensions = self.extensions.lock().await;
        extensions.insert(
            sanitized_name,
            Extension::new(config, resolved_config, Arc::from(client), server_info),
        );
        drop(extensions);
        self.invalidate_tools_cache_and_bump_version().await;

        Ok(())
    }

    pub async fn add_client(
        &self,
        name: String,
        config: ExtensionConfig,
        client: McpClientBox,
        info: Option<ServerInfo>,
    ) {
        let normalized = name_to_key(&name);
        self.extensions.lock().await.insert(
            normalized,
            Extension::new(config.clone(), config.clone(), client, info),
        );
        self.invalidate_tools_cache_and_bump_version().await;
    }

    /// Get extensions info for building the system prompt
    pub async fn get_extensions_info(&self, working_dir: &std::path::Path) -> Vec<ExtensionInfo> {
        let working_dir_str = working_dir.to_string_lossy();
        self.extensions
            .lock()
            .await
            .iter()
            .map(|(name, ext)| {
                let instructions = ext.get_instructions().unwrap_or_default();
                let instructions = instructions.replace("{{WORKING_DIR}}", &working_dir_str);
                ExtensionInfo::new(name, &instructions, ext.supports_resources())
            })
            .collect()
    }

    /// Get aggregated usage statistics
    pub async fn remove_extension(&self, name: &str) -> ExtensionResult<()> {
        let sanitized_name = name_to_key(name);
        self.remove_extension_by_key(&sanitized_name).await?;
        Ok(())
    }

    pub async fn remove_extension_by_key(&self, key: &str) -> ExtensionResult<bool> {
        let removed = self.extensions.lock().await.remove(key).is_some();
        if removed {
            self.invalidate_tools_cache_and_bump_version().await;
        }
        Ok(removed)
    }

    pub async fn update_working_dir(&self, new_dir: &std::path::Path) {
        let extensions = self.extensions.lock().await;
        for (name, ext) in extensions.iter() {
            if let Err(e) = ext.client.update_working_dir(new_dir.to_path_buf()).await {
                tracing::warn!(extension = %name, error = %e, "failed to update roots");
            }
        }
    }

    pub async fn list_extensions(&self) -> ExtensionResult<Vec<String>> {
        Ok(self.extensions.lock().await.keys().cloned().collect())
    }

    pub async fn is_extension_enabled(&self, name: &str) -> bool {
        let normalized = name_to_key(name);
        self.extensions.lock().await.contains_key(&normalized)
    }

    pub async fn get_extension_configs(&self) -> Vec<ExtensionConfig> {
        self.extensions
            .lock()
            .await
            .values()
            .map(|ext| ext.config.clone())
            .collect()
    }

    /// Get all tools from all clients with proper prefixing
    pub async fn get_prefixed_tools(
        &self,
        session_id: &str,
        extension_name: Option<String>,
    ) -> ExtensionResult<Vec<Tool>> {
        let all_tools = self.get_all_tools_cached(session_id).await?;
        Ok(self.filter_tools(&all_tools, extension_name.as_deref(), None))
    }

    pub async fn list_tools_from_extension(
        &self,
        session_id: &str,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, ErrorData> {
        let client = self
            .get_server_client(extension_name)
            .await
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("Extension {} is not valid", extension_name),
                    None,
                )
            })?;

        client
            .list_tools(session_id, None, cancellation_token)
            .await
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Unable to list tools for {}, {:?}", extension_name, e),
                    None,
                )
            })
    }

    pub async fn get_prefixed_tools_excluding(
        &self,
        session_id: &str,
        exclude: &str,
    ) -> ExtensionResult<Vec<Tool>> {
        let all_tools = self.get_all_tools_cached(session_id).await?;
        Ok(self.filter_tools(&all_tools, None, Some(exclude)))
    }

    fn filter_tools(
        &self,
        tools: &[Tool],
        extension_name: Option<&str>,
        exclude: Option<&str>,
    ) -> Vec<Tool> {
        let extension_name_normalized = extension_name.map(name_to_key);
        let exclude_normalized = exclude.map(name_to_key);

        tools
            .iter()
            .filter(|tool| {
                let tool_owner = get_tool_owner(tool)
                    .map(|s| name_to_key(&s))
                    .unwrap_or_else(|| tool.name.split("__").next().unwrap_or("").to_string());

                if let Some(ref excluded) = exclude_normalized {
                    if tool_owner == *excluded {
                        return false;
                    }
                }

                if let Some(ref name_filter) = extension_name_normalized {
                    tool_owner == *name_filter
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    }

    async fn get_all_tools_cached(&self, session_id: &str) -> ExtensionResult<Arc<Vec<Tool>>> {
        {
            let cache = self.tools_cache.lock().await;
            if let Some(ref tools) = *cache {
                return Ok(Arc::clone(tools));
            }
        }

        let version_before = self.tools_cache_version.load(Ordering::SeqCst);
        let tools = Arc::new(self.fetch_all_tools(session_id).await?);

        {
            let mut cache = self.tools_cache.lock().await;
            let version_after = self.tools_cache_version.load(Ordering::SeqCst);
            if version_after == version_before && cache.is_none() {
                *cache = Some(Arc::clone(&tools));
            }
        }

        Ok(tools)
    }

    fn host_supports_mcp_apps(&self) -> bool {
        if let Some(host_info) = &self.capabilities.host_info {
            if host_info.explicit_extensions {
                return host_info.mcpui_enabled();
            }
        }

        self.capabilities.mcpui
    }

    async fn hydrate_mcp_app_attachment(
        client: &McpClientBox,
        session_id: &str,
        resolved_tool: &ResolvedTool,
        cancellation_token: CancellationToken,
    ) -> Option<GooseMcpAppToolAttachment> {
        let resource_uri = resolved_tool.resource_uri.clone()?;

        let mut attachment = GooseMcpAppToolAttachment {
            tool_name: resolved_tool.actual_tool_name.clone(),
            tool_name_is_actual: true,
            extension_name: resolved_tool.extension_name.clone(),
            resource_uri: resource_uri.clone(),
            tool_meta: resolved_tool.tool_meta.clone(),
            resource_result: None,
            read_error: None,
        };

        match client
            .read_resource(session_id, &resource_uri, cancellation_token)
            .await
        {
            Ok(resource_result) => {
                attachment.resource_result = serde_json::to_value(&resource_result).ok();
            }
            Err(error) => {
                attachment.read_error = Some(error.to_string());
            }
        }

        Some(attachment)
    }

    pub(crate) async fn invalidate_tools_cache_and_bump_version(&self) {
        self.tools_cache_version.fetch_add(1, Ordering::SeqCst);
        *self.tools_cache.lock().await = None;
    }

    async fn fetch_all_tools(&self, session_id: &str) -> ExtensionResult<Vec<Tool>> {
        let clients: Vec<_> = self
            .extensions
            .lock()
            .await
            .iter()
            .map(|(name, ext)| (name.clone(), ext.config.clone(), ext.get_client()))
            .collect();

        let cancel_token = CancellationToken::default();
        let client_futures = clients.into_iter().map(|(name, config, client)| {
            let cancel_token = cancel_token.clone();
            let ext_name = name.clone();
            async move {
                let mut tools = Vec::new();
                let mut client_tools = match client
                    .list_tools(session_id, None, cancel_token.clone())
                    .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(extension = %ext_name, error = %e, "Failed to list tools");
                        return (name, vec![]);
                    }
                };

                let expose_unprefixed = is_unprefixed_extension(&config);

                loop {
                    for mut tool in client_tools.tools {
                        if config.is_tool_available(&tool.name) {
                            let public_name = if expose_unprefixed {
                                tool.name.to_string()
                            } else {
                                format!("{}__{}", name, tool.name)
                            };

                            let mut meta_map = tool
                                .meta
                                .as_ref()
                                .map(|m| m.0.clone())
                                .unwrap_or_default();
                            meta_map.insert(
                                TOOL_EXTENSION_META_KEY.to_string(),
                                serde_json::Value::String(name.clone()),
                            );

                            tool.name = public_name.into();
                            tool.meta = Some(rmcp::model::MetaObject(meta_map));

                            let mut schema = (*tool.input_schema).clone();
                            if super::tool_schema_normalize::normalize_input_schema(
                                &mut schema,
                            ) {
                                tool.input_schema = Arc::new(schema);
                            }

                            tools.push(tool);
                        }
                    }

                    if client_tools.next_cursor.is_none() {
                        break;
                    }

                    client_tools = match client
                        .list_tools(session_id, client_tools.next_cursor, cancel_token.clone())
                        .await
                    {
                        Ok(t) => t,
                        Err(e) => {
                            warn!(extension = %ext_name, error = %e, "Failed to list tools (pagination)");
                            break;
                        }
                    };
                }

                (name, tools)
            }
        });

        let results = future::join_all(client_futures).await;

        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut tools = Vec::new();
        for (ext_name, client_tools) in results {
            for tool in client_tools {
                let tool_name = tool.name.to_string();
                if seen_names.contains(&tool_name) {
                    warn!(
                        tool = %tool_name,
                        extension = %ext_name,
                        "Duplicate tool name - skipping"
                    );
                    continue;
                }
                seen_names.insert(tool_name);
                tools.push(tool);
            }
        }

        Ok(tools)
    }

    // Function that gets executed for read_resource tool
    pub async fn read_resource_tool(
        &self,
        session_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<ContentBlock>, ErrorData> {
        let uri = require_str_parameter(&params, "uri")?;
        let extension_name = require_str_parameter(&params, "extension_name")?;

        let read_result = self
            .read_resource(session_id, uri, extension_name, cancellation_token)
            .await?;

        let mut result = Vec::new();
        for content in read_result.contents {
            if let ResourceContents::TextResourceContents { text, .. } = content {
                result.push(ContentBlock::text(format!("{}\n\n{}", uri, text)));
            }
        }
        Ok(result)
    }

    pub async fn read_resource(
        &self,
        session_id: &str,
        uri: &str,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> Result<rmcp::model::ReadResourceResult, ErrorData> {
        let available_extensions = self
            .extensions
            .lock()
            .await
            .keys()
            .map(|s| s.as_str())
            .collect::<Vec<&str>>()
            .join(", ");
        let error_msg = format!(
            "Extension '{}' not found. Here are the available extensions: {}",
            extension_name, available_extensions
        );

        let client = self
            .get_server_client(extension_name)
            .await
            .ok_or(ErrorData::new(ErrorCode::INVALID_PARAMS, error_msg, None))?;

        client
            .read_resource(session_id, uri, cancellation_token)
            .await
            .map_err(|_| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Could not read resource with uri: {}", uri),
                    None,
                )
            })
    }

    pub async fn get_ui_resources(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, Resource)>, ErrorData> {
        let mut ui_resources = Vec::new();

        let extensions_to_check: Vec<(String, McpClientBox)> = {
            let extensions = self.extensions.lock().await;
            extensions
                .iter()
                .map(|(name, ext)| (name.clone(), ext.get_client()))
                .collect()
        };

        for (extension_name, client) in extensions_to_check {
            match client
                .list_resources(session_id, None, CancellationToken::default())
                .await
            {
                Ok(list_response) => {
                    for resource in list_response.resources {
                        if resource.uri.starts_with("ui://") {
                            ui_resources.push((extension_name.clone(), resource));
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to list resources for {}: {:?}", extension_name, e);
                }
            }
        }

        Ok(ui_resources)
    }

    pub async fn list_resources_result_from_extension(
        &self,
        session_id: &str,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> Result<ListResourcesResult, ErrorData> {
        let client = self
            .get_server_client(extension_name)
            .await
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("Extension {} is not valid", extension_name),
                    None,
                )
            })?;

        client
            .list_resources(session_id, None, cancellation_token)
            .await
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Unable to list resources for {}, {:?}", extension_name, e),
                    None,
                )
            })
    }

    async fn list_resources_from_extension(
        &self,
        session_id: &str,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<ContentBlock>, ErrorData> {
        self.list_resources_result_from_extension(session_id, extension_name, cancellation_token)
            .await
            .map(|lr| {
                let resource_list = lr
                    .resources
                    .into_iter()
                    .map(|r| format!("{} - {}, uri: ({})", extension_name, r.name, r.uri))
                    .collect::<Vec<String>>()
                    .join("\n");

                vec![ContentBlock::text(resource_list)]
            })
    }

    pub async fn list_resources(
        &self,
        session_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<ContentBlock>, ErrorData> {
        let extension = params.get("extension_name").and_then(|v| v.as_str());

        match extension {
            Some(extension_name) => {
                // Handle single extension case
                self.list_resources_from_extension(session_id, extension_name, cancellation_token)
                    .await
            }
            None => {
                // Handle all extensions case using FuturesUnordered
                let mut futures = FuturesUnordered::new();

                // Create futures for each resource_capable_extension
                self.extensions
                    .lock()
                    .await
                    .iter()
                    .filter(|(_name, ext)| ext.supports_resources())
                    .map(|(name, _ext)| name.clone())
                    .for_each(|name| {
                        let token = cancellation_token.clone();
                        futures.push(async move {
                            self.list_resources_from_extension(session_id, name.as_str(), token)
                                .await
                        });
                    });

                let mut all_resources = Vec::new();
                let mut errors = Vec::new();

                // Process results as they complete
                while let Some(result) = futures.next().await {
                    match result {
                        Ok(content) => {
                            all_resources.extend(content);
                        }
                        Err(tool_error) => {
                            errors.push(tool_error);
                        }
                    }
                }

                if !errors.is_empty() {
                    tracing::error!(
                        errors = ?errors
                            .into_iter()
                            .map(|e| format!("{:?}", e))
                            .collect::<Vec<_>>(),
                        "errors from listing resources"
                    );
                }

                Ok(all_resources)
            }
        }
    }

    #[cfg(test)]
    async fn resolve_tool(
        &self,
        session_id: &str,
        tool_name: &str,
    ) -> Result<ResolvedTool, ErrorData> {
        self.resolve_tool_with_constraints(session_id, tool_name, None, false)
            .await
    }

    async fn resolve_tool_with_constraints(
        &self,
        session_id: &str,
        tool_name: &str,
        expected_extension_name: Option<&str>,
        require_app_visibility: bool,
    ) -> Result<ResolvedTool, ErrorData> {
        let tools = self.get_all_tools_cached(session_id).await.map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to get tools: {}", e),
                None,
            )
        })?;

        let mut name = tool_name.to_string();
        let mut recovery_attempted = false;
        loop {
            if let Some(tool) = tools.iter().find(|t| *t.name == *name) {
                let owner = get_tool_owner(tool)
                    .or_else(|| name.split_once("__").map(|(prefix, _)| name_to_key(prefix)))
                    .ok_or_else(|| {
                        ErrorData::new(
                            ErrorCode::RESOURCE_NOT_FOUND,
                            format!("Tool '{}' has no owner", name),
                            None,
                        )
                    })?;

                if expected_extension_name
                    .is_some_and(|expected| name_to_key(expected) != name_to_key(&owner))
                {
                    return Err(ErrorData::new(
                        ErrorCode::RESOURCE_NOT_FOUND,
                        format!("Tool '{}' not found for extension", tool_name),
                        None,
                    ));
                }

                if require_app_visibility && !is_tool_visible_to_app(tool) {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Tool is not visible to app clients",
                        None,
                    ));
                }

                let actual_tool_name = name
                    .strip_prefix(&format!("{owner}__"))
                    .unwrap_or(&name)
                    .to_string();

                let client = self.get_server_client(&owner).await.ok_or_else(|| {
                    ErrorData::new(
                        ErrorCode::RESOURCE_NOT_FOUND,
                        format!("Extension '{}' not found for tool '{}'", owner, name),
                        None,
                    )
                })?;

                return Ok(ResolvedTool {
                    extension_name: owner,
                    actual_tool_name,
                    client,
                    tool_meta: get_tool_meta_value(tool),
                    resource_uri: get_tool_resource_uri(tool),
                });
            }

            if !recovery_attempted {
                recovery_attempted = true;
                let owners: Vec<(&str, Option<String>)> = tools
                    .iter()
                    .map(|t| (t.name.as_ref(), get_tool_owner(t)))
                    .collect();
                if let Some(recovered) =
                    recover_mangled_tool_name(&name, owners.iter().map(|(n, o)| (*n, o.as_deref())))
                {
                    name = recovered;
                    continue;
                }
            }

            break;
        }

        let available = tools
            .iter()
            .map(|t| t.name.as_ref())
            .collect::<Vec<&str>>()
            .join(", ");

        Err(ErrorData::new(
            ErrorCode::RESOURCE_NOT_FOUND,
            format!(
                "Tool '{}' not found. Available tools: [{}]",
                tool_name, available
            ),
            None,
        ))
    }

    pub async fn dispatch_tool_call(
        &self,
        ctx: &super::tool_execution::ToolCallContext,
        tool_call: CallToolRequestParams,
        cancellation_token: CancellationToken,
    ) -> std::result::Result<ToolCallResult, ErrorData> {
        self.dispatch_tool_call_inner(ctx, tool_call, None, false, cancellation_token)
            .await
    }

    pub async fn dispatch_app_tool_call(
        &self,
        ctx: &super::tool_execution::ToolCallContext,
        tool_call: CallToolRequestParams,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> std::result::Result<ToolCallResult, ErrorData> {
        self.dispatch_tool_call_inner(
            ctx,
            tool_call,
            Some(extension_name),
            true,
            cancellation_token,
        )
        .await
    }

    async fn dispatch_tool_call_inner(
        &self,
        ctx: &super::tool_execution::ToolCallContext,
        tool_call: CallToolRequestParams,
        expected_extension_name: Option<&str>,
        require_app_visibility: bool,
        cancellation_token: CancellationToken,
    ) -> std::result::Result<ToolCallResult, ErrorData> {
        let tool_name_str = tool_call.name.to_string();
        let resolved = self
            .resolve_tool_with_constraints(
                &ctx.session_id,
                &tool_name_str,
                expected_extension_name,
                require_app_visibility,
            )
            .await?;

        if let Some(extension) = self.extensions.lock().await.get(&resolved.extension_name) {
            if !extension
                .config
                .is_tool_available(&resolved.actual_tool_name)
            {
                return Err(ErrorData::new(
                    ErrorCode::RESOURCE_NOT_FOUND,
                    format!(
                        "Tool '{}' is not available for extension '{}'",
                        resolved.actual_tool_name, resolved.extension_name
                    ),
                    None,
                ));
            }
        }

        let arguments = tool_call.arguments.clone();
        let client = resolved.client.clone();
        let hydration_client = client.clone();
        let client_notifications_receiver = client.subscribe().await;
        let session_id = ctx.session_id.clone();
        let action_required_tool_call_request_id = ctx.tool_call_request_id.clone();
        let action_required_manager = self.context.session_manager.action_required();
        let action_required_receiver =
            if let Some(tool_call_request_id) = action_required_tool_call_request_id.clone() {
                if action_required_manager
                    .has_action_required_stream(&session_id, &tool_call_request_id)
                    .await
                {
                    None
                } else {
                    let registered_tool_call_request_id = tool_call_request_id.clone();
                    let receiver = action_required_manager
                        .register_action_required_stream(session_id.clone(), tool_call_request_id)
                        .await;
                    Some((
                        receiver,
                        session_id.clone(),
                        registered_tool_call_request_id,
                    ))
                }
            } else {
                None
            };
        let actual_tool_name = resolved.actual_tool_name.clone();
        let resolved_tool = resolved;
        let should_hydrate_mcp_app = self.host_supports_mcp_apps();
        let read_cancellation_token = cancellation_token.clone();
        let owned_ctx = ToolCallContext::new(
            ctx.session_id.clone(),
            ctx.working_dir.clone(),
            ctx.tool_call_request_id.clone(),
        );
        let (owned_ctx, tool_call_notifications_receiver) =
            if let Some(notification_emitter) = ctx.notification_emitter().cloned() {
                (
                    owned_ctx.with_notification_emitter(notification_emitter),
                    None,
                )
            } else if owned_ctx.tool_call_request_id.is_some() {
                let (tool_call_notifications_sender, tool_call_notifications_receiver) =
                    mpsc::channel(TOOL_CALL_NOTIFICATION_CHANNEL_CAPACITY);
                (
                    owned_ctx.with_notification_emitter(ToolCallNotificationEmitter::new(
                        tool_call_notifications_sender,
                    )),
                    Some(tool_call_notifications_receiver),
                )
            } else {
                (owned_ctx, None)
            };
        let notification_stream: Box<dyn Stream<Item = ServerNotification> + Send + Unpin> =
            match tool_call_notifications_receiver {
                Some(tool_call_notifications_receiver) => Box::new(stream::select(
                    ReceiverStream::new(client_notifications_receiver),
                    ReceiverStream::new(tool_call_notifications_receiver),
                )),
                None => Box::new(ReceiverStream::new(client_notifications_receiver)),
            };

        let fut = async move {
            tracing::debug!(
                "dispatch_tool_call: calling client.call_tool tool={} session_id={} working_dir={:?}",
                actual_tool_name,
                owned_ctx.session_id,
                owned_ctx.working_dir,
            );
            let call_result = client
                .call_tool(&owned_ctx, &actual_tool_name, arguments, cancellation_token)
                .await
                .map_err(|e| match e {
                    ServiceError::McpError(error_data) => error_data,
                    _ => {
                        ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), e.maybe_to_value())
                    }
                });

            let mut result = call_result?;

            remove_untrusted_mcp_app_meta(&mut result);

            if should_hydrate_mcp_app && result.is_error != Some(true) {
                if let Some(attachment) = Self::hydrate_mcp_app_attachment(
                    &hydration_client,
                    &session_id,
                    &resolved_tool,
                    read_cancellation_token,
                )
                .await
                {
                    insert_trusted_tool_update_meta(&mut result, &attachment);
                }
            }

            Ok(result)
        };

        Ok(ToolCallResult {
            result: Box::new(fut.boxed()),
            notification_stream: Some(notification_stream),
            action_required_stream: action_required_receiver.map(
                |(rx, session_id, tool_call_request_id)| {
                    Box::new(ActionRequiredStream::new(
                        rx,
                        action_required_manager,
                        session_id,
                        tool_call_request_id,
                    )) as _
                },
            ),
        })
    }

    pub async fn list_prompts_from_extension(
        &self,
        session_id: &str,
        extension_name: &str,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<Prompt>, ErrorData> {
        let client = self
            .get_server_client(extension_name)
            .await
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("Extension {} is not valid", extension_name),
                    None,
                )
            })?;

        client
            .list_prompts(session_id, None, cancellation_token)
            .await
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Unable to list prompts for {}, {:?}", extension_name, e),
                    None,
                )
            })
            .map(|lp| lp.prompts)
    }

    pub async fn list_prompts(
        &self,
        session_id: &str,
        cancellation_token: CancellationToken,
    ) -> Result<HashMap<String, Vec<Prompt>>, ErrorData> {
        let mut futures = FuturesUnordered::new();

        let names: Vec<_> = self.extensions.lock().await.keys().cloned().collect();
        for extension_name in names {
            let token = cancellation_token.clone();
            futures.push(async move {
                (
                    extension_name.clone(),
                    self.list_prompts_from_extension(session_id, extension_name.as_str(), token)
                        .await,
                )
            });
        }

        let mut all_prompts = HashMap::new();
        let mut errors = Vec::new();

        // Process results as they complete
        while let Some(result) = futures.next().await {
            let (name, prompts) = result;
            match prompts {
                Ok(content) => {
                    all_prompts.insert(name.to_string(), content);
                }
                Err(tool_error) => {
                    errors.push(tool_error);
                }
            }
        }

        if !errors.is_empty() {
            tracing::debug!(
                errors = ?errors
                    .into_iter()
                    .map(|e| format!("{:?}", e))
                    .collect::<Vec<_>>(),
                "errors from listing prompts"
            );
        }

        Ok(all_prompts)
    }

    pub async fn get_prompt(
        &self,
        session_id: &str,
        extension_name: &str,
        name: &str,
        arguments: Value,
        cancellation_token: CancellationToken,
    ) -> Result<GetPromptResult> {
        let client = self
            .get_server_client(extension_name)
            .await
            .ok_or_else(|| anyhow::anyhow!("Extension {} not found", extension_name))?;

        client
            .get_prompt(session_id, name, arguments, cancellation_token)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get prompt: {}", e))
    }

    pub async fn search_available_extensions(&self) -> Result<Vec<ContentBlock>, ErrorData> {
        let mut output_parts = vec![];

        // First get disabled extensions from current config (skip hidden ones)
        let mut disabled_extensions: Vec<String> = vec![];
        for extension in get_all_extensions() {
            if !extension.enabled && !is_hidden_extension(&extension.config.name()) {
                let config = extension.config.clone();
                let description = match &config {
                    ExtensionConfig::Builtin {
                        description,
                        display_name,
                        ..
                    } => {
                        if description.is_empty() {
                            display_name.as_deref().unwrap_or("Built-in extension")
                        } else {
                            description
                        }
                    }
                    ExtensionConfig::Platform { description, .. }
                    | ExtensionConfig::StreamableHttp { description, .. }
                    | ExtensionConfig::Stdio { description, .. } => description,
                };
                disabled_extensions.push(format!("- {} - {}", config.name(), description));
            }
        }

        // Get currently enabled extensions that can be disabled (skip hidden ones)
        let enabled_extensions: Vec<String> = self
            .extensions
            .lock()
            .await
            .keys()
            .filter(|name| !is_hidden_extension(name))
            .cloned()
            .collect();

        // Build output string
        if !disabled_extensions.is_empty() {
            output_parts.push(format!(
                "Extensions available to enable:\n{}\n",
                disabled_extensions.join("\n")
            ));
        } else {
            output_parts.push("No extensions available to enable.\n".to_string());
        }

        if !enabled_extensions.is_empty() {
            output_parts.push(format!(
                "\n\nExtensions available to disable:\n{}\n",
                enabled_extensions
                    .iter()
                    .map(|name| format!("- {}", name))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        } else {
            output_parts.push("No extensions that can be disabled.\n".to_string());
        }

        Ok(vec![ContentBlock::text(output_parts.join("\n"))])
    }

    async fn get_server_client(&self, name: impl Into<String>) -> Option<McpClientBox> {
        let normalized = name_to_key(&name.into());
        self.extensions
            .lock()
            .await
            .get(&normalized)
            .map(|ext| ext.get_client())
    }

    pub async fn collect_moim_parts(&self, session_id: &str) -> Vec<String> {
        let mut platform_clients: Vec<(String, McpClientBox)> = {
            let extensions = self.extensions.lock().await;
            extensions
                .iter()
                .filter_map(|(name, extension)| {
                    let is_platform = match &extension.config {
                        ExtensionConfig::Platform { .. } => true,
                        ExtensionConfig::Builtin { name: ext_name, .. } => {
                            PLATFORM_EXTENSIONS.contains_key(name_to_key(ext_name).as_str())
                        }
                        _ => false,
                    };
                    if is_platform {
                        Some((name.clone(), extension.get_client()))
                    } else {
                        None
                    }
                })
                .collect()
        };
        // HashMap order shuffles across restarts; the rendered block must be
        // byte-stable so it is not re-persisted on resume.
        platform_clients.sort_by(|a, b| a.0.cmp(&b.0));

        let mut parts = Vec::new();
        for (name, client) in platform_clients {
            if let Some(moim_content) = client.get_moim(session_id).await {
                tracing::debug!("MOIM content from {}: {} chars", name, moim_content.len());
                parts.push(moim_content);
            }
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolResult;
    use rmcp::model::{CustomNotification, InitializeResult, JsonObject};
    use rmcp::{object, ServiceError as Error};

    use rmcp::model::ListPromptsResult;
    use rmcp::model::ListResourcesResult;
    use rmcp::model::ListToolsResult;
    use rmcp::model::ReadResourceResult;
    use rmcp::model::ServerNotification;

    use tokio::sync::mpsc;

    impl ExtensionManager {
        async fn add_mock_extension(&self, name: String, client: McpClientBox) {
            self.add_mock_extension_with_tools(name, client, vec![])
                .await;
        }

        async fn add_mock_extension_with_tools(
            &self,
            name: String,
            client: McpClientBox,
            available_tools: Vec<String>,
        ) {
            let sanitized_name = name_to_key(&name);
            let config = ExtensionConfig::Builtin {
                name: name.clone(),
                display_name: Some(name.clone()),
                description: "built-in".to_string(),
                timeout: None,
                bundled: None,
                available_tools,
            };
            let extension = Extension::new(config.clone(), config.clone(), client, None);
            self.extensions
                .lock()
                .await
                .insert(sanitized_name, extension);
            self.invalidate_tools_cache_and_bump_version().await;
        }
    }

    struct MockClient {}

    #[async_trait::async_trait]
    impl McpClientTrait for MockClient {
        fn get_info(&self) -> Option<&InitializeResult> {
            None
        }

        async fn list_resources(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListResourcesResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn read_resource(
            &self,
            _session_id: &str,
            _uri: &str,
            _cancellation_token: CancellationToken,
        ) -> Result<ReadResourceResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn list_tools(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListToolsResult, Error> {
            use serde_json::json;
            use std::sync::Arc;
            Ok(ListToolsResult {
                tools: vec![
                    Tool::new(
                        "tool".to_string(),
                        "A basic tool".to_string(),
                        Arc::new(json!({}).as_object().unwrap().clone()),
                    ),
                    Tool::new(
                        "available_tool".to_string(),
                        "An available tool".to_string(),
                        Arc::new(json!({}).as_object().unwrap().clone()),
                    ),
                    Tool::new(
                        "hidden_tool".to_string(),
                        "hidden tool".to_string(),
                        Arc::new(json!({}).as_object().unwrap().clone()),
                    ),
                    {
                        let mut t = Tool::new(
                            "render_chart".to_string(),
                            "Render a chart".to_string(),
                            Arc::new(json!({}).as_object().unwrap().clone()),
                        );
                        t.meta = Some(MetaObject(
                            json!({ "ui": { "resourceUri": "ui://autovisualiser/chart" } })
                                .as_object()
                                .unwrap()
                                .clone(),
                        ));
                        t
                    },
                ],
                next_cursor: None,
                meta: None,
                ..Default::default()
            })
        }

        async fn call_tool(
            &self,
            _ctx: &ToolCallContext,
            name: &str,
            _arguments: Option<JsonObject>,
            _cancellation_token: CancellationToken,
        ) -> Result<CallToolResult, Error> {
            match name {
                "tool" | "test__tool" | "available_tool" | "hidden_tool" | "render_chart"
                | "unadvertised_tool" => Ok(CallToolResult::success(vec![])),
                _ => Err(Error::TransportClosed),
            }
        }

        async fn list_prompts(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListPromptsResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn get_prompt(
            &self,
            _session_id: &str,
            _name: &str,
            _arguments: Value,
            _cancellation_token: CancellationToken,
        ) -> Result<GetPromptResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
            mpsc::channel(1).1
        }
    }

    struct ContextNotificationClient;

    #[async_trait::async_trait]
    impl McpClientTrait for ContextNotificationClient {
        fn get_info(&self) -> Option<&InitializeResult> {
            None
        }

        async fn list_tools(
            &self,
            session_id: &str,
            next_cursor: Option<String>,
            cancellation_token: CancellationToken,
        ) -> Result<ListToolsResult, Error> {
            MockClient {}
                .list_tools(session_id, next_cursor, cancellation_token)
                .await
        }

        async fn call_tool(
            &self,
            ctx: &ToolCallContext,
            _name: &str,
            _arguments: Option<JsonObject>,
            _cancellation_token: CancellationToken,
        ) -> Result<CallToolResult, Error> {
            if let Some(emitter) = ctx.notification_emitter() {
                let request_id = ctx
                    .tool_call_request_id
                    .as_deref()
                    .expect("an emitter requires a request ID");
                emitter.emit_best_effort(ServerNotification::CustomNotification(
                    CustomNotification::new(format!("scoped/{request_id}"), None),
                ));
            }
            Ok(CallToolResult::success(vec![]))
        }

        async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
            let (sender, receiver) = mpsc::channel(1);
            sender
                .try_send(ServerNotification::CustomNotification(
                    CustomNotification::new("client/subscription", None),
                ))
                .expect("test notification should fit");
            receiver
        }
    }

    async fn dispatch_notification_methods(ctx: ToolCallContext) -> Vec<String> {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension(
                "notifications".to_string(),
                Arc::new(ContextNotificationClient),
            )
            .await;

        let tool_call = CallToolRequestParams::new("notifications__tool".to_string())
            .with_arguments(object!({}));
        let dispatched = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await
            .expect("tool call should dispatch");

        assert!(dispatched.result.await.is_ok());

        let mut methods = dispatched
            .notification_stream
            .expect("notification stream should exist")
            .filter_map(|notification| async move {
                match notification {
                    ServerNotification::CustomNotification(notification) => {
                        Some(notification.method)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .await;
        methods.sort();
        methods
    }

    #[tokio::test]
    async fn dispatch_merges_request_scoped_and_client_notifications() {
        let methods = dispatch_notification_methods(ToolCallContext::new(
            "session".to_string(),
            None,
            Some("request".to_string()),
        ))
        .await;

        assert_eq!(methods, vec!["client/subscription", "scoped/request"]);
    }

    #[tokio::test]
    async fn dispatch_reuses_existing_notification_emitter() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension(
                "notifications".to_string(),
                Arc::new(ContextNotificationClient),
            )
            .await;
        let (sender, mut receiver) = mpsc::channel(1);
        let ctx = ToolCallContext::new(
            "nested-session".to_string(),
            None,
            Some("nested-request".to_string()),
        )
        .with_notification_emitter(ToolCallNotificationEmitter::new(sender));
        let tool_call = CallToolRequestParams::new("notifications__tool".to_string())
            .with_arguments(object!({}));

        let dispatched = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await
            .expect("tool call should dispatch");
        assert!(dispatched.result.await.is_ok());

        let notification = receiver
            .try_recv()
            .expect("parent emitter should receive nested notification");
        let ServerNotification::CustomNotification(notification) = notification else {
            panic!("expected a custom notification");
        };
        assert_eq!(notification.method, "scoped/nested-request");

        let methods = dispatched
            .notification_stream
            .expect("client notification stream should exist")
            .filter_map(|notification| async move {
                match notification {
                    ServerNotification::CustomNotification(notification) => {
                        Some(notification.method)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .await;
        assert_eq!(methods, vec!["client/subscription"]);
    }

    #[tokio::test]
    async fn dispatch_without_request_id_uses_only_client_notifications() {
        let methods =
            dispatch_notification_methods(ToolCallContext::new("session".to_string(), None, None))
                .await;

        assert_eq!(methods, vec!["client/subscription"]);
    }

    #[tokio::test]
    async fn test_dispatch_tool_call() {
        use super::super::tool_execution::ToolCallContext;

        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        // Add some mock clients using the helper method
        extension_manager
            .add_mock_extension("test_client".to_string(), Arc::new(MockClient {}))
            .await;

        extension_manager
            .add_mock_extension("__cli__ent__".to_string(), Arc::new(MockClient {}))
            .await;

        extension_manager
            .add_mock_extension("client 🚀".to_string(), Arc::new(MockClient {}))
            .await;

        let ctx = ToolCallContext::new(
            "test-session-id".to_string(),
            None,
            Some("test-req-id".to_string()),
        );

        let tool_call =
            CallToolRequestParams::new("test_client__tool".to_string()).with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await;
        assert!(result.is_ok());

        let tool_call = CallToolRequestParams::new("test_client__available_tool".to_string())
            .with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await;
        assert!(result.is_ok());

        let tool_call = CallToolRequestParams::new("__cli__ent____tool".to_string())
            .with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await;
        assert!(result.is_ok());

        let tool_call =
            CallToolRequestParams::new("client___tool".to_string()).with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await;
        assert!(result.is_ok());

        let invalid_tool_call =
            CallToolRequestParams::new("client___tools".to_string()).with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, invalid_tool_call, CancellationToken::default())
            .await;
        if let Err(err) = result {
            assert_eq!(err.code, ErrorCode::RESOURCE_NOT_FOUND);
        } else {
            panic!("Expected ErrorData with ErrorCode::RESOURCE_NOT_FOUND");
        }

        let invalid_tool_call =
            CallToolRequestParams::new("_client__tools".to_string()).with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, invalid_tool_call, CancellationToken::default())
            .await;
        if let Err(err) = result {
            assert_eq!(err.code, ErrorCode::RESOURCE_NOT_FOUND);
        } else {
            panic!("Expected ErrorData with ErrorCode::RESOURCE_NOT_FOUND");
        }
    }

    #[tokio::test]
    async fn test_tool_availability_filtering() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        // Only "available_tool" should be available to the LLM
        let available_tools = vec!["available_tool".to_string()];

        extension_manager
            .add_mock_extension_with_tools(
                "test_extension".to_string(),
                Arc::new(MockClient {}),
                available_tools,
            )
            .await;

        let tools = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(!tool_names.iter().any(|name| name == "test_extension__tool")); // Default unavailable
        assert!(tool_names
            .iter()
            .any(|name| name == "test_extension__available_tool"));
        assert!(!tool_names
            .iter()
            .any(|name| name == "test_extension__hidden_tool"));
        assert!(tool_names.len() == 1);
    }

    #[tokio::test]
    async fn test_tool_availability_defaults_to_available() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension_with_tools(
                "test_extension".to_string(),
                Arc::new(MockClient {}),
                vec![], // Empty available_tools means all tools are available by default
            )
            .await;

        let tools = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();

        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(tool_names.iter().any(|name| name == "test_extension__tool"));
        assert!(tool_names
            .iter()
            .any(|name| name == "test_extension__available_tool"));
        assert!(tool_names
            .iter()
            .any(|name| name == "test_extension__hidden_tool"));
        assert!(tool_names
            .iter()
            .any(|name| name == "test_extension__render_chart"));
        assert!(tool_names.len() == 4);
    }

    #[tokio::test]
    async fn test_dispatch_unavailable_tool_returns_error() {
        use super::super::tool_execution::ToolCallContext;

        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        let available_tools = vec!["available_tool".to_string()];

        extension_manager
            .add_mock_extension_with_tools(
                "test_extension".to_string(),
                Arc::new(MockClient {}),
                available_tools,
            )
            .await;

        let ctx = ToolCallContext::new(
            "test-session-id".to_string(),
            None,
            Some("test-req-id".to_string()),
        );

        let unavailable_tool_call = CallToolRequestParams::new("test_extension__tool".to_string())
            .with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, unavailable_tool_call, CancellationToken::default())
            .await;

        if let Err(err) = result {
            assert_eq!(err.code, ErrorCode::RESOURCE_NOT_FOUND);
        } else {
            panic!("Expected ErrorData with ErrorCode::RESOURCE_NOT_FOUND");
        }

        // Try to call an available tool - should succeed
        let available_tool_call =
            CallToolRequestParams::new("test_extension__available_tool".to_string())
                .with_arguments(object!({}));

        let result = extension_manager
            .dispatch_tool_call(&ctx, available_tool_call, CancellationToken::default())
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_streamable_http_header_env_substitution() {
        let mut env_map = HashMap::new();
        env_map.insert("AUTH_TOKEN".to_string(), "secret123".to_string());
        env_map.insert("API_KEY".to_string(), "key456".to_string());

        // Test ${VAR} syntax
        let result = substitute_env_vars("Bearer ${ AUTH_TOKEN }", &env_map);
        assert_eq!(result, "Bearer secret123");

        // Test ${VAR} syntax without spaces
        let result = substitute_env_vars("Bearer ${AUTH_TOKEN}", &env_map);
        assert_eq!(result, "Bearer secret123");

        // Test $VAR syntax
        let result = substitute_env_vars("Bearer $AUTH_TOKEN", &env_map);
        assert_eq!(result, "Bearer secret123");

        // Test multiple substitutions
        let result = substitute_env_vars("Key: $API_KEY, Token: ${AUTH_TOKEN}", &env_map);
        assert_eq!(result, "Key: key456, Token: secret123");

        // Test no substitution when variable doesn't exist
        let result = substitute_env_vars("Bearer ${UNKNOWN_VAR}", &env_map);
        assert_eq!(result, "Bearer ${UNKNOWN_VAR}");

        // Test mixed content
        let result = substitute_env_vars(
            "Authorization: Bearer ${AUTH_TOKEN} and API ${API_KEY}",
            &env_map,
        );
        assert_eq!(result, "Authorization: Bearer secret123 and API key456");
    }

    #[tokio::test]
    async fn test_substitute_env_vars_no_recursive_expansion() {
        let mut env_map = HashMap::new();
        env_map.insert("TOKEN".to_string(), "abc$KEY".to_string());
        env_map.insert("KEY".to_string(), "xyz".to_string());

        // A substituted value containing $KEY should NOT be re-expanded
        let result = substitute_env_vars("${TOKEN}", &env_map);
        assert_eq!(result, "abc$KEY");

        let result = substitute_env_vars("$TOKEN", &env_map);
        assert_eq!(result, "abc$KEY");
    }

    #[tokio::test]
    async fn test_tools_cache_invalidated_on_add_extension() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("ext_a".to_string(), Arc::new(MockClient {}))
            .await;

        let tools_after_first = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();
        let tool_names: Vec<String> = tools_after_first
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(!tool_names.iter().any(|n| n.starts_with("ext_b__")));

        extension_manager
            .add_mock_extension("ext_b".to_string(), Arc::new(MockClient {}))
            .await;

        let tools_after_second = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();
        let tool_names: Vec<String> = tools_after_second
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert!(tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(tool_names.iter().any(|n| n.starts_with("ext_b__")));
    }

    #[tokio::test]
    async fn test_tools_cache_invalidated_on_remove_extension() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("ext_a".to_string(), Arc::new(MockClient {}))
            .await;
        extension_manager
            .add_mock_extension("ext_b".to_string(), Arc::new(MockClient {}))
            .await;

        let tools_before = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();
        let tool_names: Vec<String> = tools_before.iter().map(|t| t.name.to_string()).collect();
        assert!(tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(tool_names.iter().any(|n| n.starts_with("ext_b__")));

        extension_manager.remove_extension("ext_b").await.unwrap();

        let tools_after = extension_manager
            .get_prefixed_tools("test-session-id", None)
            .await
            .unwrap();
        let tool_names: Vec<String> = tools_after.iter().map(|t| t.name.to_string()).collect();
        assert!(tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(!tool_names.iter().any(|n| n.starts_with("ext_b__")));
    }

    #[tokio::test]
    async fn test_get_prefixed_tools_excluding() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("ext_a".to_string(), Arc::new(MockClient {}))
            .await;
        extension_manager
            .add_mock_extension("ext_b".to_string(), Arc::new(MockClient {}))
            .await;

        let tools = extension_manager
            .get_prefixed_tools_excluding("test-session-id", "ext_a")
            .await
            .unwrap();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        assert!(!tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(tool_names.iter().any(|n| n.starts_with("ext_b__")));
    }

    #[tokio::test]
    async fn test_mcp_app_tools_identified_for_code_mode_exclusion() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("autovisualiser".to_string(), Arc::new(MockClient {}))
            .await;

        let tools = extension_manager
            .get_prefixed_tools_excluding("test-session-id", "code_execution")
            .await
            .unwrap();

        let (mcp_app_tools, regular_tools): (Vec<_>, Vec<_>) = tools
            .iter()
            .partition(|t| get_tool_resource_uri(t).is_some());

        assert_eq!(mcp_app_tools.len(), 1, "exactly one MCP app tool");
        assert_eq!(
            mcp_app_tools[0].name.as_ref(),
            "autovisualiser__render_chart"
        );
        assert!(
            regular_tools
                .iter()
                .all(|t| get_tool_resource_uri(t).is_none()),
            "non-MCP-app tools have no resourceUri"
        );
    }

    #[tokio::test]
    async fn test_get_prefixed_tools_by_extension_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("ext_a".to_string(), Arc::new(MockClient {}))
            .await;
        extension_manager
            .add_mock_extension("ext_b".to_string(), Arc::new(MockClient {}))
            .await;

        let tools = extension_manager
            .get_prefixed_tools("test-session-id", Some("ext_a".to_string()))
            .await
            .unwrap();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        assert!(tool_names.iter().any(|n| n.starts_with("ext_a__")));
        assert!(!tool_names.iter().any(|n| n.starts_with("ext_b__")));
    }

    #[test]
    fn test_tool_owner_binding_uses_metadata_not_flattened_name() {
        let tool = |name: &str, owner: &str| {
            let mut tool = Tool::new(
                name.to_string(),
                "test tool".to_string(),
                Arc::new(serde_json::Map::new()),
            );
            tool.meta = Some(MetaObject(
                serde_json::json!({ TOOL_EXTENSION_META_KEY: owner })
                    .as_object()
                    .unwrap()
                    .clone(),
            ));
            tool
        };

        let own_tool = tool("ext_a__own", "ext_a");
        let sibling_tool = tool("ext_a__ext_b__secret", "ext_a__ext_b");

        assert!(is_tool_owned_by_extension(&own_tool, "ext_a"));
        assert!(!is_tool_owned_by_extension(&sibling_tool, "ext_a"));
    }

    #[tokio::test]
    async fn app_dispatch_revalidates_owner_after_tools_cache_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("ext_a__ext_b".to_string(), Arc::new(MockClient {}))
            .await;

        let app_tool = |owner: &str| {
            let mut tool = Tool::new(
                "ext_a__ext_b__secret".to_string(),
                "test tool".to_string(),
                Arc::new(serde_json::Map::new()),
            );
            tool.meta = Some(MetaObject(
                serde_json::json!({
                    TOOL_EXTENSION_META_KEY: owner,
                    "ui": { "resourceUri": "ui://test/app" }
                })
                .as_object()
                .unwrap()
                .clone(),
            ));
            tool
        };

        *extension_manager.tools_cache.lock().await = Some(Arc::new(vec![app_tool("ext_a")]));
        let initially_visible = extension_manager
            .get_prefixed_tools("session", Some("ext_a".to_string()))
            .await
            .unwrap();
        assert_eq!(initially_visible.len(), 1);

        // Model tools/list_changed replacing the validated tool with a sibling
        // owner's colliding flattened name before the actual dispatch.
        *extension_manager.tools_cache.lock().await =
            Some(Arc::new(vec![app_tool("ext_a__ext_b")]));
        let ctx = ToolCallContext::new("session".to_string(), None, None);
        let result = extension_manager
            .dispatch_app_tool_call(
                &ctx,
                CallToolRequestParams::new("ext_a__ext_b__secret".to_string()),
                "ext_a",
                CancellationToken::default(),
            )
            .await;

        let Err(error) = result else {
            panic!("app dispatch accepted a sibling owner's colliding tool name");
        };
        assert_eq!(error.code, ErrorCode::RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_resolve_tool_error_includes_available_tools() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());

        extension_manager
            .add_mock_extension("ext_a".to_string(), Arc::new(MockClient {}))
            .await;

        let result = extension_manager
            .resolve_tool("test-session-id", "definitely_not_a_real_tool")
            .await;
        let err = match result {
            Ok(_) => panic!("resolve_tool should fail for an unknown name"),
            Err(e) => e,
        };

        let msg = err.message.to_string();
        assert!(
            msg.contains("definitely_not_a_real_tool"),
            "error should echo the bad name; got: {msg}"
        );
        assert!(
            msg.contains("ext_a__"),
            "error should list at least one real tool name; got: {msg}"
        );
    }

    struct MockDottedClient {}

    #[async_trait::async_trait]
    impl McpClientTrait for MockDottedClient {
        fn get_info(&self) -> Option<&InitializeResult> {
            None
        }

        async fn list_resources(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListResourcesResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn read_resource(
            &self,
            _session_id: &str,
            _uri: &str,
            _cancellation_token: CancellationToken,
        ) -> Result<ReadResourceResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn list_tools(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListToolsResult, Error> {
            use serde_json::json;
            use std::sync::Arc;
            Ok(ListToolsResult {
                tools: vec![
                    Tool::new(
                        "db.query".to_string(),
                        "A tool with a dotted name".to_string(),
                        Arc::new(json!({}).as_object().unwrap().clone()),
                    ),
                    Tool::new(
                        "db__query".to_string(),
                        "A sibling with the separator name".to_string(),
                        Arc::new(json!({}).as_object().unwrap().clone()),
                    ),
                ],
                next_cursor: None,
                meta: None,
                ..Default::default()
            })
        }

        async fn call_tool(
            &self,
            _ctx: &ToolCallContext,
            name: &str,
            _arguments: Option<JsonObject>,
            _cancellation_token: CancellationToken,
        ) -> Result<CallToolResult, Error> {
            match name {
                "db.query" | "db__query" => Ok(CallToolResult::success(vec![])),
                _ => Err(Error::TransportClosed),
            }
        }

        async fn list_prompts(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListPromptsResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn get_prompt(
            &self,
            _session_id: &str,
            _name: &str,
            _arguments: Value,
            _cancellation_token: CancellationToken,
        ) -> Result<GetPromptResult, Error> {
            Err(Error::TransportClosed)
        }

        async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
            mpsc::channel(1).1
        }
    }

    #[tokio::test]
    async fn test_resolve_tool_recovers_dotted_mangled_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("test_client".to_string(), Arc::new(MockClient {}))
            .await;

        let resolved = extension_manager
            .resolve_tool("test-session-id", "test_client.tool")
            .await
            .expect("mangled dotted name should resolve to the real tool");
        assert_eq!(resolved.extension_name, "test_client");
        assert_eq!(resolved.actual_tool_name, "tool");
    }

    #[tokio::test]
    async fn test_resolve_tool_recovers_functions_prefixed_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("test_client".to_string(), Arc::new(MockClient {}))
            .await;

        let resolved = extension_manager
            .resolve_tool("test-session-id", "functions.test_client__tool")
            .await
            .expect("functions-prefixed name should resolve to the real tool");
        assert_eq!(resolved.extension_name, "test_client");
        assert_eq!(resolved.actual_tool_name, "tool");
    }

    #[tokio::test]
    async fn test_resolve_tool_exact_dotted_name_never_rewritten() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("dotted".to_string(), Arc::new(MockDottedClient {}))
            .await;

        let resolved = extension_manager
            .resolve_tool("test-session-id", "dotted__db.query")
            .await
            .expect("exact dotted tool name must resolve");
        assert_eq!(resolved.extension_name, "dotted");
        assert_eq!(resolved.actual_tool_name, "db.query");
    }

    #[tokio::test]
    async fn test_resolve_tool_recovers_mangled_separator_with_dotted_tool_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("dotted".to_string(), Arc::new(MockDottedClient {}))
            .await;

        let resolved = extension_manager
            .resolve_tool("test-session-id", "dotted.db.query")
            .await
            .expect("mangled extension separator should resolve");
        assert_eq!(resolved.extension_name, "dotted");
        assert_eq!(resolved.actual_tool_name, "db.query");
    }

    #[tokio::test]
    async fn test_resolve_tool_recovers_unprefixed_platform_extension_name() {
        // GLM's documented reproduction (#9486): the built-in "developer"
        // platform extension is registered with unprefixed_tools=true, so its
        // tools are advertised with no "__" prefix at all (owner only in
        // metadata). "developer.tool" must still resolve to the real "tool".
        // Naming the mock extension literally "developer" makes
        // is_unprefixed_extension look it up in the real PLATFORM_EXTENSIONS
        // registry, exercising production behavior, not a fake stand-in.
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("developer".to_string(), Arc::new(MockClient {}))
            .await;

        let resolved = extension_manager
            .resolve_tool("test-session-id", "developer.tool")
            .await
            .expect("unprefixed extension namespace mangling should resolve");
        assert_eq!(resolved.actual_tool_name, "tool");
        assert_eq!(resolved.extension_name, "developer");
    }

    #[tokio::test]
    async fn test_dispatch_rejects_unadvertised_tool_implemented_by_extension() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager =
            ExtensionManager::new_without_provider(temp_dir.path().to_path_buf());
        extension_manager
            .add_mock_extension("test_client".to_string(), Arc::new(MockClient {}))
            .await;

        let ctx = ToolCallContext::new("test-session-id".to_string(), None, None);
        let tool_call = CallToolRequestParams::new("test_client__unadvertised_tool".to_string());

        let err = match extension_manager
            .dispatch_tool_call(&ctx, tool_call, CancellationToken::default())
            .await
        {
            Ok(_) => panic!("an unadvertised tool must not be dispatched"),
            Err(err) => err,
        };

        assert_eq!(err.code, ErrorCode::RESOURCE_NOT_FOUND);
    }

    #[test]
    fn test_recover_mangled_tool_name() {
        let tools = [("developer__shell", None), ("platform__search", None)];
        assert_eq!(
            recover_mangled_tool_name("developer.shell", tools.iter().copied()).as_deref(),
            Some("developer__shell")
        );
        assert_eq!(
            recover_mangled_tool_name("functions.developer__shell", tools.iter().copied())
                .as_deref(),
            Some("developer__shell")
        );
        assert_eq!(
            recover_mangled_tool_name("functions.developer.shell", tools.iter().copied())
                .as_deref(),
            Some("developer__shell")
        );
        assert_eq!(
            recover_mangled_tool_name("developer shell", tools.iter().copied()),
            None
        );
        assert_eq!(
            recover_mangled_tool_name("developer__shell!", tools.iter().copied()),
            None
        );
        assert_eq!(
            recover_mangled_tool_name("nonexistent.tool", tools.iter().copied()),
            None
        );

        let dotted_tool = [("dotted__db.query", None)];
        assert_eq!(
            recover_mangled_tool_name("dotted.db.query", dotted_tool.iter().copied()).as_deref(),
            Some("dotted__db.query")
        );
    }

    #[test]
    fn test_recover_mangled_tool_name_unprefixed_extension() {
        // Platform extensions with unprefixed_tools=true (e.g. "developer")
        // advertise tools with no "__" prefix at all; the owner lives only in
        // metadata. GLM's documented "developer.shell" reproduction (#9486)
        // and emulated "developer__shell" calls must recover via the owner,
        // not the tool's own (absent) prefix.
        let tools = [("shell", Some("developer")), ("write", Some("developer"))];
        assert_eq!(
            recover_mangled_tool_name("developer.shell", tools.iter().copied()).as_deref(),
            Some("shell")
        );
        assert_eq!(
            recover_mangled_tool_name("developer__shell", tools.iter().copied()).as_deref(),
            Some("shell")
        );
        assert_eq!(
            recover_mangled_tool_name("functions.developer.shell", tools.iter().copied())
                .as_deref(),
            Some("shell")
        );

        // Wrong owner must not match.
        assert_eq!(
            recover_mangled_tool_name("other_extension.shell", tools.iter().copied()),
            None
        );

        // Ambiguity across two different unprefixed extensions that both own
        // a tool matching the same mangled input must refuse, not guess.
        let ambiguous = [("shell", Some("dev_a")), ("shell", Some("dev_b"))];
        assert_eq!(
            recover_mangled_tool_name("dev_a.shell", ambiguous.iter().copied()).as_deref(),
            Some("shell")
        );
    }

    #[test]
    fn test_recover_mangled_tool_name_non_extension_manager_tools() {
        // recipe__final_output and platform__manage_schedule are appended by
        // Agent::list_tools outside the extension manager (see #9486); they
        // use the same "__" convention, so no owner metadata is needed.
        let tools = [
            ("recipe__final_output", None),
            ("platform__manage_schedule", None),
        ];
        assert_eq!(
            recover_mangled_tool_name("recipe.final_output", tools.iter().copied()).as_deref(),
            Some("recipe__final_output")
        );
        assert_eq!(
            recover_mangled_tool_name("platform.manage_schedule", tools.iter().copied()).as_deref(),
            Some("platform__manage_schedule")
        );
    }

    #[test]
    fn test_remove_untrusted_mcp_app_meta_strips_spoofed_payload() {
        let mut result = CallToolResult::success(vec![]);
        result.meta = Some(MetaObject(
            serde_json::from_value(serde_json::json!({
                "goose": {
                    "mcpApp": {
                        "resourceUri": "ui://spoofed/app",
                    },
                    "other": true,
                },
                TRUSTED_TOOL_UPDATE_META_KEY: {
                    "mcpApp": {
                        "resourceUri": "ui://spoofed/internal",
                    },
                },
            }))
            .unwrap(),
        ));

        remove_untrusted_mcp_app_meta(&mut result);

        let meta = result.meta.expect("expected remaining meta");
        assert_eq!(meta.0.get(TRUSTED_TOOL_UPDATE_META_KEY), None);
        assert_eq!(
            meta.0.get("goose"),
            Some(&serde_json::json!({ "other": true }))
        );
    }

    #[test]
    fn test_insert_trusted_tool_update_meta_stores_backend_payload() {
        let mut result = CallToolResult::success(vec![]);
        let attachment = GooseMcpAppToolAttachment {
            tool_name: "render__secret".to_string(),
            tool_name_is_actual: true,
            extension_name: "weather".to_string(),
            resource_uri: "ui://weather/app".to_string(),
            tool_meta: None,
            resource_result: Some(serde_json::json!({
                "contents": [
                    {
                        "uri": "ui://weather/app",
                        "mimeType": "text/html;profile=mcp-app",
                        "text": "<div>Hello</div>",
                    },
                ],
            })),
            read_error: None,
        };

        insert_trusted_tool_update_meta(&mut result, &attachment);

        let meta = result.meta.expect("expected trusted meta");
        assert_eq!(
            meta.0.get(TRUSTED_TOOL_UPDATE_META_KEY),
            Some(&serde_json::json!({
                "mcpApp": {
                    "toolName": "render__secret",
                    "toolNameIsActual": true,
                    "extensionName": "weather",
                    "resourceUri": "ui://weather/app",
                    "resourceResult": {
                        "contents": [
                            {
                                "uri": "ui://weather/app",
                                "mimeType": "text/html;profile=mcp-app",
                                "text": "<div>Hello</div>",
                            },
                        ],
                    },
                },
            })),
        );
    }

    #[tokio::test]
    async fn test_add_extension_noop_on_identical_config() {
        // When add_extension is called with a config that is byte-for-byte identical to
        // the already-loaded one, it must return Ok(()) without removing the extension.
        let temp_dir = tempfile::tempdir().unwrap();
        let em = Arc::new(ExtensionManager::new_without_provider(
            temp_dir.path().to_path_buf(),
        ));

        let config = ExtensionConfig::Platform {
            name: "test-ext".to_string(),
            description: "original".to_string(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        };

        em.add_client(
            "test-ext".to_string(),
            config.clone(),
            Arc::new(MockClient {}),
            None,
        )
        .await;
        assert_eq!(em.extensions.lock().await.len(), 1);

        // Calling add_extension with the same config must be a no-op (Ok, count unchanged).
        let result = em.add_extension(config, None, None, None).await;
        assert!(result.is_ok(), "identical config should be a no-op");
        assert_eq!(
            em.extensions.lock().await.len(),
            1,
            "extension must not be removed on no-op"
        );
    }

    #[tokio::test]
    async fn test_add_extension_replaces_extension_on_config_change() {
        // When add_extension is called with an updated config (same name, different fields),
        // the existing extension must be removed so the caller can re-add with new config.
        let temp_dir = tempfile::tempdir().unwrap();
        let em = Arc::new(ExtensionManager::new_without_provider(
            temp_dir.path().to_path_buf(),
        ));

        let config_a = ExtensionConfig::Platform {
            name: "test-ext".to_string(),
            description: "version-a".to_string(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        };
        let config_b = ExtensionConfig::Platform {
            name: "test-ext".to_string(),
            description: "version-b".to_string(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        };

        em.add_client(
            "test-ext".to_string(),
            config_a,
            Arc::new(MockClient {}),
            None,
        )
        .await;
        assert_eq!(em.extensions.lock().await.len(), 1);

        let result = em.add_extension(config_b, None, None, None).await;
        assert!(
            result.is_err(),
            "unknown platform extension must return Err"
        );
        assert_eq!(
            em.extensions.lock().await.len(),
            1,
            "old extension must be preserved when replacement client creation fails"
        );
    }
}
