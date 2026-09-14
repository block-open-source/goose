use crate::action_required_manager::{ActionRequiredManager, ElicitationOutcome};
use crate::agents::extension_manager::ExtensionManager;
use crate::agents::tool_execution::ToolCallContext;
use crate::agents::types::SharedProvider;
use crate::session_context::{SESSION_ID_HEADER, TOOL_CALL_REQUEST_ID_HEADER, WORKING_DIR_HEADER};
/// MCP client implementation for Goose
#[expect(deprecated)]
use rmcp::model::{CreateMessageRequestParams, CreateMessageResult, SamplingMessage};
#[expect(deprecated)]
use rmcp::model::{
    ElicitRequestParams, ElicitResult, ListRootsResult, LoggingMessageNotification, Root,
    SamplingMessageContentBlock,
};
use rmcp::model::{
    ElicitationAction, ErrorCode, ExtensionCapabilities, Extensions, JsonObject, MetaObject,
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, CancelledNotificationParam, ClientCapabilities,
        ClientInfo, ClientRequest, GetPromptRequestParams, GetPromptResult, Implementation,
        InitializeRequestParams, InitializeResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult, Notification, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResult, Request, RequestId, RequestOptionalParam,
        Role, ServerNotification, ServerResult,
    },
    service::{
        ClientInitializeError, ClientLifecycleMode, ClientServiceExt, PeerRequestOptions,
        RequestContext, RequestHandle, RunningService, ServiceRole,
    },
    transport::IntoTransport,
    ClientHandler, ErrorData, Peer, RoleClient, ServiceError,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::Duration,
};
use tokio::sync::{
    mpsc::{self, Sender},
    Mutex,
};
use tokio_util::sync::CancellationToken;

pub type BoxError = Box<dyn std::error::Error + Sync + Send>;

pub type Error = rmcp::ServiceError;

const MCP_APPS_UI_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
const MCP_APPS_UI_MIME_TYPE: &str = "text/html;profile=mcp-app";

fn extract_sampling_text(
    content: &[crate::conversation::message::MessageContent],
) -> Option<String> {
    let visible_content = content
        .iter()
        .filter_map(crate::conversation::message::MessageContent::user_visible_content)
        .collect::<Vec<_>>();
    let text = visible_content
        .iter()
        .filter_map(|content| match content {
            crate::conversation::message::MessageContent::Text(text)
                if !text.text.trim().is_empty() =>
            {
                Some(text.text.as_str())
            }
            _ => None,
        })
        .collect::<String>();

    (!text.is_empty()).then_some(text)
}

fn resolve_sampling_model_config() -> anyhow::Result<goose_providers::model::ModelConfig> {
    let config = crate::config::Config::global();
    let provider_name = config.get_goose_provider()?;
    let model_name = config.get_goose_model()?;
    crate::model_config::model_config_from_user_config(&provider_name, &model_name)
}

fn default_mcp_apps_ui_extensions() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    let mut ui_extension_settings = JsonObject::new();
    ui_extension_settings.insert(
        "mimeTypes".to_string(),
        serde_json::json!([MCP_APPS_UI_MIME_TYPE]),
    );
    extensions.insert(MCP_APPS_UI_EXTENSION_ID.to_string(), ui_extension_settings);
    extensions
}

#[derive(Debug, Clone, Default)]
pub struct GooseMcpHostInfo {
    pub explicit_extensions: bool,
    pub extensions: ExtensionCapabilities,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
}

impl GooseMcpHostInfo {
    pub fn mcpui_enabled(&self) -> bool {
        self.extensions.contains_key(MCP_APPS_UI_EXTENSION_ID)
    }
}

#[async_trait::async_trait]
pub trait McpClientTrait: Send + Sync {
    async fn list_tools(
        &self,
        session_id: &str,
        next_cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<ListToolsResult, Error>;

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancel_token: CancellationToken,
    ) -> Result<CallToolResult, Error>;

    fn get_info(&self) -> Option<&InitializeResult>;

    /// Return the extension's current instructions. The default reads from
    /// `get_info()`, but platform extensions can override this to provide
    /// dynamically computed instructions (e.g. freshly discovered skills).
    fn get_instructions(&self) -> Option<String> {
        self.get_info().and_then(|info| info.instructions.clone())
    }

    async fn list_resources(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListResourcesResult, Error> {
        Err(Error::TransportClosed)
    }

    async fn read_resource(
        &self,
        _session_id: &str,
        _uri: &str,
        _cancel_token: CancellationToken,
    ) -> Result<ReadResourceResult, Error> {
        Err(Error::TransportClosed)
    }

    async fn list_prompts(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListPromptsResult, Error> {
        Err(Error::TransportClosed)
    }

    async fn get_prompt(
        &self,
        _session_id: &str,
        _name: &str,
        _arguments: Value,
        _cancel_token: CancellationToken,
    ) -> Result<GetPromptResult, Error> {
        Err(Error::TransportClosed)
    }

    async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
        mpsc::channel(1).1
    }

    async fn get_moim(&self, _session_id: &str) -> Option<String> {
        None
    }

    async fn update_working_dir(&self, _new_dir: PathBuf) -> Result<(), Error> {
        Ok(())
    }
}

struct ActiveToolCallGuard {
    active_tool_calls: Arc<StdMutex<HashMap<String, Vec<String>>>>,
    session_id: String,
    tool_call_request_id: String,
}

impl Drop for ActiveToolCallGuard {
    fn drop(&mut self) {
        let mut active_tool_calls = self
            .active_tool_calls
            .lock()
            .expect("active_tool_calls mutex poisoned");
        if let Some(calls) = active_tool_calls.get_mut(&self.session_id) {
            if let Some(pos) = calls.iter().position(|id| id == &self.tool_call_request_id) {
                calls.remove(pos);
            }
            if calls.is_empty() {
                active_tool_calls.remove(&self.session_id);
            }
        }
    }
}

pub struct GooseClient {
    notification_handlers: Arc<Mutex<Vec<Sender<ServerNotification>>>>,
    provider: SharedProvider,
    session_id: Mutex<Option<String>>,
    active_tool_calls: Arc<StdMutex<HashMap<String, Vec<String>>>>,
    client_name: String,
    capabilities: GooseMcpClientCapabilities,
    working_dir: Arc<tokio::sync::RwLock<PathBuf>>,
    action_required: Arc<ActionRequiredManager>,
    extension_manager: Weak<ExtensionManager>,
}

impl GooseClient {
    pub(crate) fn new(
        handlers: Arc<Mutex<Vec<Sender<ServerNotification>>>>,
        provider: SharedProvider,
        client_name: String,
        capabilities: GooseMcpClientCapabilities,
        working_dir: PathBuf,
        action_required: Arc<ActionRequiredManager>,
        extension_manager: Weak<ExtensionManager>,
    ) -> Self {
        GooseClient {
            notification_handlers: handlers,
            provider,
            session_id: Mutex::new(None),
            active_tool_calls: Arc::new(StdMutex::new(HashMap::new())),
            client_name,
            capabilities,
            working_dir: Arc::new(tokio::sync::RwLock::new(working_dir)),
            action_required,
            extension_manager,
        }
    }

    pub fn shared_working_dir(&self) -> Arc<tokio::sync::RwLock<PathBuf>> {
        self.working_dir.clone()
    }

    async fn set_session_id(&self, session_id: &str) {
        let mut slot = self.session_id.lock().await;
        assert!(
            slot.as_deref().is_none_or(|s| s == session_id),
            "McpClient received requests from different sessions"
        );
        *slot = Some(session_id.to_string());
    }

    async fn handle_tool_list_changed(&self) {
        if let Some(extension_manager) = self.extension_manager.upgrade() {
            extension_manager
                .invalidate_tools_cache_and_bump_version()
                .await;
        }
    }

    async fn current_session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    async fn resolve_session_id(&self, extensions: &Extensions) -> Option<String> {
        // Prefer explicit MCP metadata, then the active request scope.
        let current_session_id = self.current_session_id().await;
        Self::session_id_from_extensions(extensions).or(current_session_id)
    }

    fn session_id_from_extensions(extensions: &Extensions) -> Option<String> {
        let meta = extensions.get::<MetaObject>()?;
        meta.0
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(SESSION_ID_HEADER))
            .and_then(|(_, value)| value.as_str())
            .map(|value| value.to_string())
    }

    fn tool_call_request_id_from_extensions(extensions: &Extensions) -> Option<String> {
        let meta = extensions.get::<MetaObject>()?;
        meta.0
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(TOOL_CALL_REQUEST_ID_HEADER))
            .and_then(|(_, value)| value.as_str())
            .map(|value| value.to_string())
    }

    fn register_active_tool_call(
        &self,
        session_id: &str,
        tool_call_request_id: &str,
    ) -> ActiveToolCallGuard {
        self.active_tool_calls
            .lock()
            .expect("active_tool_calls mutex poisoned")
            .entry(session_id.to_string())
            .or_default()
            .push(tool_call_request_id.to_string());
        ActiveToolCallGuard {
            active_tool_calls: self.active_tool_calls.clone(),
            session_id: session_id.to_string(),
            tool_call_request_id: tool_call_request_id.to_string(),
        }
    }

    fn resolve_tool_call_request_id(
        &self,
        session_id: &str,
        extensions: &Extensions,
    ) -> Result<String, ErrorData> {
        if let Some(tool_call_request_id) = Self::tool_call_request_id_from_extensions(extensions) {
            return Ok(tool_call_request_id);
        }

        let active_tool_calls = self
            .active_tool_calls
            .lock()
            .expect("active_tool_calls mutex poisoned");
        match active_tool_calls.get(session_id).map(Vec::as_slice) {
            Some([tool_call_request_id]) => Ok(tool_call_request_id.clone()),
            Some(calls) if calls.len() > 1 => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Cannot correlate elicitation request: multiple tool calls are active and the \
                 server did not echo the tool call request id",
                None,
            )),
            _ => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Could not resolve tool call request id for elicitation request",
                None,
            )),
        }
    }

    fn resolved_extensions(&self) -> ExtensionCapabilities {
        if let Some(host_info) = &self.capabilities.host_info {
            if host_info.explicit_extensions {
                return host_info.extensions.clone();
            }
        }

        if self.capabilities.mcpui {
            return default_mcp_apps_ui_extensions();
        }

        ExtensionCapabilities::new()
    }

    fn resolved_client_info(&self) -> Implementation {
        let name = self
            .capabilities
            .host_info
            .as_ref()
            .and_then(|host_info| host_info.client_name.clone())
            .unwrap_or_else(|| self.client_name.clone());
        let version = self
            .capabilities
            .host_info
            .as_ref()
            .and_then(|host_info| host_info.client_version.clone())
            .unwrap_or_else(|| {
                std::env::var("GOOSE_MCP_CLIENT_VERSION")
                    .unwrap_or(env!("CARGO_PKG_VERSION").to_owned())
            });

        Implementation::new(name, version)
    }
}

#[expect(deprecated)]
fn working_dir_roots(dir: &std::path::Path) -> ListRootsResult {
    let uri = url::Url::from_file_path(dir)
        .map(|u| u.to_string())
        .unwrap_or_else(|()| format!("file://{}", dir.display()));
    ListRootsResult::new(vec![Root::new(uri).with_name("working_directory")])
}

/// Fan out a notification to all subscribers, dropping senders whose receivers are gone.
fn fan_out_notification(
    handlers: &mut Vec<Sender<ServerNotification>>,
    notification: ServerNotification,
) {
    handlers.retain(|handler| match handler.try_send(notification.clone()) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    });
}

impl ClientHandler for GooseClient {
    #[expect(deprecated)]
    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        Ok(working_dir_roots(&self.working_dir.read().await))
    }

    async fn on_progress(
        &self,
        params: rmcp::model::ProgressNotificationParam,
        context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        let mut not = Notification::new(params);
        not.extensions = context.extensions;
        fan_out_notification(
            &mut *self.notification_handlers.lock().await,
            ServerNotification::ProgressNotification(not),
        );
    }

    async fn on_tool_list_changed(&self, _context: rmcp::service::NotificationContext<RoleClient>) {
        self.handle_tool_list_changed().await;
    }

    #[expect(deprecated)]
    async fn on_logging_message(
        &self,
        params: rmcp::model::LoggingMessageNotificationParam,
        context: rmcp::service::NotificationContext<rmcp::RoleClient>,
    ) {
        let mut notification = LoggingMessageNotification::new(params);
        notification.extensions = context.extensions;
        fan_out_notification(
            &mut *self.notification_handlers.lock().await,
            ServerNotification::LoggingMessageNotification(notification),
        );
    }

    #[expect(deprecated)]
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, ErrorData> {
        let provider = self
            .provider
            .lock()
            .await
            .as_ref()
            .ok_or(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Could not use provider",
                None,
            ))?
            .clone();

        // Prefer explicit MCP metadata, then the active request scope.
        let session_id = self.resolve_session_id(&context.extensions).await;

        let provider_ready_messages: Vec<crate::conversation::message::Message> = params
            .messages
            .iter()
            .map(|msg| {
                let base = match msg.role {
                    Role::User => crate::conversation::message::Message::user(),
                    Role::Assistant => crate::conversation::message::Message::assistant(),
                };

                match msg.content.first().and_then(|c| c.as_text()) {
                    Some(text) => base.with_text(&text.text),
                    None => base,
                }
            })
            .collect();

        let system_prompt = params
            .system_prompt
            .as_deref()
            .unwrap_or("You are a general-purpose AI agent called goose");

        let model_config = resolve_sampling_model_config().map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Could not resolve model config",
                Some(Value::from(e.to_string())),
            )
        })?;
        let (response, usage) = crate::session_context::with_session_id(
            session_id.clone(),
            provider.complete(&model_config, system_prompt, &provider_ready_messages, &[]),
        )
        .await
        .map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Unexpected error while completing the prompt",
                Some(Value::from(e.to_string())),
            )
        })?;

        let sampling_content = if let Some(text) = extract_sampling_text(&response.content) {
            SamplingMessageContentBlock::text(text)
        } else if let Some(crate::conversation::message::MessageContent::Image(img)) = response
            .content
            .iter()
            .filter_map(crate::conversation::message::MessageContent::user_visible_content)
            .find(|content| {
                matches!(
                    content,
                    crate::conversation::message::MessageContent::Image(_)
                )
            })
        {
            SamplingMessageContentBlock::Image(rmcp::model::ImageContent::new(
                img.data.clone(),
                img.mime_type.clone(),
            ))
        } else {
            return Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "Provider returned no usable text or image content for sampling",
                None,
            ));
        };

        Ok(CreateMessageResult::new(
            SamplingMessage::new(Role::Assistant, sampling_content),
            usage.model,
        )
        .with_stop_reason(CreateMessageResult::STOP_REASON_END_TURN))
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let Some(handler) = &self.capabilities.elicitation_handler {
            return Ok(handler(&request));
        }

        let session_id = self
            .resolve_session_id(&context.extensions)
            .await
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Could not resolve session id for elicitation request",
                    None,
                )
            })?;
        let tool_call_request_id =
            self.resolve_tool_call_request_id(&session_id, &context.extensions)?;

        let (message, schema_value) = match &request {
            ElicitRequestParams::FormElicitationParams {
                message,
                requested_schema,
                ..
            } => {
                let schema_value = serde_json::to_value(requested_schema).map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to serialize elicitation schema: {}", e),
                        None,
                    )
                })?;
                (message.clone(), schema_value)
            }
            ElicitRequestParams::UrlElicitationParams { message, url, .. } => {
                (message.clone(), serde_json::json!({ "url": url }))
            }
            _ => (String::new(), serde_json::json!({})),
        };

        self.action_required
            .request_and_wait(
                session_id,
                tool_call_request_id,
                message,
                schema_value,
                Duration::from_secs(300),
            )
            .await
            .map(|response| match response {
                ElicitationOutcome::Accept(user_data) => {
                    ElicitResult::new(ElicitationAction::Accept).with_content(user_data)
                }
                ElicitationOutcome::Decline => ElicitResult::new(ElicitationAction::Decline),
                ElicitationOutcome::Cancel => ElicitResult::new(ElicitationAction::Cancel),
            })
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Elicitation request timed out or failed: {}", e),
                    None,
                )
            })
    }

    fn get_info(&self) -> ClientInfo {
        let extensions = self.resolved_extensions();

        InitializeRequestParams::new(
            #[expect(deprecated)]
            ClientCapabilities::builder()
                .enable_roots()
                .enable_extensions_with(extensions)
                .enable_sampling()
                .enable_elicitation()
                .build(),
            self.resolved_client_info(),
        )
        .with_protocol_version(
            self.capabilities
                .protocol_version
                .clone()
                .unwrap_or_default(),
        )
    }
}

pub type ElicitationHandler = Arc<dyn Fn(&ElicitRequestParams) -> ElicitResult + Send + Sync>;

#[derive(Clone, Default)]
pub struct GooseMcpClientCapabilities {
    pub mcpui: bool,
    pub host_info: Option<GooseMcpHostInfo>,
    pub elicitation_handler: Option<ElicitationHandler>,
    pub protocol_version: Option<ProtocolVersion>,
}

impl std::fmt::Debug for GooseMcpClientCapabilities {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GooseMcpClientCapabilities")
            .field("mcpui", &self.mcpui)
            .field("host_info", &self.host_info)
            .field("elicitation_handler", &self.elicitation_handler.is_some())
            .field("protocol_version", &self.protocol_version)
            .finish()
    }
}

/// The MCP client is the interface for MCP operations.
pub struct McpClient {
    client: Mutex<Arc<RunningService<RoleClient, GooseClient>>>,
    notification_subscribers: Arc<Mutex<Vec<mpsc::Sender<ServerNotification>>>>,
    server_info: Option<InitializeResult>,
    timeout: std::time::Duration,
    docker_container: Option<String>,
}

impl McpClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn connect<T, E, A>(
        transport: T,
        timeout: std::time::Duration,
        provider: SharedProvider,
        client_name: String,
        capabilities: GooseMcpClientCapabilities,
        working_dir: PathBuf,
        action_required: Arc<ActionRequiredManager>,
        extension_manager: Weak<ExtensionManager>,
    ) -> Result<Self, ClientInitializeError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
    {
        Self::connect_with_container(
            transport,
            timeout,
            provider,
            None,
            client_name,
            capabilities,
            working_dir,
            action_required,
            extension_manager,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn connect_with_container<T, E, A>(
        transport: T,
        timeout: std::time::Duration,
        provider: SharedProvider,
        docker_container: Option<String>,
        client_name: String,
        capabilities: GooseMcpClientCapabilities,
        working_dir: PathBuf,
        action_required: Arc<ActionRequiredManager>,
        extension_manager: Weak<ExtensionManager>,
    ) -> Result<Self, ClientInitializeError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
    {
        let notification_subscribers =
            Arc::new(Mutex::new(Vec::<mpsc::Sender<ServerNotification>>::new()));

        let client = GooseClient::new(
            notification_subscribers.clone(),
            provider,
            client_name.clone(),
            capabilities.clone(),
            working_dir,
            action_required,
            extension_manager,
        );
        let client: rmcp::service::RunningService<rmcp::RoleClient, GooseClient> =
            if let Some(protocol_version) = capabilities.protocol_version {
                let lifecycle = if protocol_version >= ProtocolVersion::STANDARD_HEADERS {
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![protocol_version],
                    }
                } else {
                    ClientLifecycleMode::Initialize
                };
                client.serve_with_lifecycle(transport, lifecycle).await?
            } else {
                client
                    .serve_with_lifecycle(
                        transport,
                        ClientLifecycleMode::Auto {
                            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                            legacy_version: Some(ProtocolVersion::V_2025_11_25),
                        },
                    )
                    .await?
            };
        let server_info = client.peer_info().map(|info| {
            let mut initialize_result = InitializeResult::new(info.capabilities.clone())
                .with_protocol_version(info.protocol_version.clone());
            if let Some(server_info) = &info.server_info {
                initialize_result = initialize_result.with_server_info(server_info.clone());
            }
            initialize_result.instructions = info.instructions.clone();
            initialize_result.meta = info.meta.clone();
            initialize_result
        });

        Ok(Self {
            client: Mutex::new(Arc::new(client)),
            notification_subscribers,
            server_info,
            timeout,
            docker_container,
        })
    }

    pub fn docker_container(&self) -> Option<&str> {
        self.docker_container.as_deref()
    }

    async fn do_update_working_dir(&self, new_dir: PathBuf) -> Result<(), Error> {
        let client = self.client.lock().await;
        let shared = client.service().shared_working_dir();
        *shared.write().await = new_dir;
        client.peer().notify_roots_list_changed().await?;
        Ok(())
    }

    async fn send_request_with_context(
        &self,
        session_id: &str,
        working_dir: Option<&str>,
        tool_call_request_id: Option<&str>,
        request: ClientRequest,
        cancel_token: CancellationToken,
    ) -> Result<ServerResult, Error> {
        let request = inject_session_context_into_request(
            request,
            Some(session_id),
            working_dir,
            tool_call_request_id,
        );
        let active_tool_call = tool_call_request_id.filter(|id| !id.is_empty());
        // The inner mutex is held only for the send; the actual response wait
        // happens outside the lock so concurrent calls can overlap. The guard
        // unregisters the active tool call on drop, covering cancellation and
        // dropped reply streams as well as normal completion.
        let (handle, _active_tool_call_guard) = {
            let client = self.client.lock().await;
            client.service().set_session_id(session_id).await;
            let guard = active_tool_call.map(|tool_call_request_id| {
                client
                    .service()
                    .register_active_tool_call(session_id, tool_call_request_id)
            });
            let handle = client
                .send_cancellable_request(request, PeerRequestOptions::no_options())
                .await?;
            (handle, guard)
        };

        await_response(handle, self.timeout, &cancel_token).await
    }
}

async fn await_response(
    handle: RequestHandle<RoleClient>,
    timeout: Duration,
    cancel_token: &CancellationToken,
) -> Result<<RoleClient as ServiceRole>::PeerResp, ServiceError> {
    let receiver = handle.rx;
    let peer = handle.peer;
    let request_id = handle.id;
    tokio::select! {
        result = receiver => {
            result.map_err(|_e| ServiceError::TransportClosed)?
        }
        _ = tokio::time::sleep(timeout) => {
            send_cancel_message(&peer, request_id, Some("timed out".to_owned())).await?;
            Err(ServiceError::Timeout{timeout})
        }
        _ = cancel_token.cancelled() => {
            send_cancel_message(&peer, request_id, Some("operation cancelled".to_owned())).await?;
            Err(ServiceError::Cancelled { reason: None })
        }
    }
}

async fn send_cancel_message(
    peer: &Peer<RoleClient>,
    request_id: RequestId,
    reason: Option<String>,
) -> Result<(), ServiceError> {
    peer.send_notification(
        Notification::new(CancelledNotificationParam::new(Some(request_id), reason)).into(),
    )
    .await
}

#[async_trait::async_trait]
impl McpClientTrait for McpClient {
    fn get_info(&self) -> Option<&InitializeResult> {
        self.server_info.as_ref()
    }

    async fn list_resources(
        &self,
        session_id: &str,
        cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<ListResourcesResult, Error> {
        let res = self
            .send_request_with_context(
                session_id,
                None,
                None,
                ClientRequest::ListResourcesRequest(RequestOptionalParam::with_param(
                    PaginatedRequestParams::default().with_cursor(cursor),
                )),
                cancel_token,
            )
            .await?;

        match res {
            ServerResult::ListResourcesResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn read_resource(
        &self,
        session_id: &str,
        uri: &str,
        cancel_token: CancellationToken,
    ) -> Result<ReadResourceResult, Error> {
        let params = ReadResourceRequestParams::new(uri.to_string());
        let client = self.client.lock().await.clone();
        if client
            .peer_info()
            .is_some_and(|info| info.protocol_version == ProtocolVersion::V_2026_07_28)
        {
            client.service().set_session_id(session_id).await;
            return tokio::select! {
                result = client.read_resource(params) => result,
                _ = tokio::time::sleep(self.timeout) => Err(ServiceError::Timeout { timeout: self.timeout }),
                _ = cancel_token.cancelled() => Err(ServiceError::Cancelled { reason: None }),
            };
        }
        drop(client);

        let res = self
            .send_request_with_context(
                session_id,
                None,
                None,
                ClientRequest::ReadResourceRequest(Request::new(params)),
                cancel_token,
            )
            .await?;

        match res {
            ServerResult::ReadResourceResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn list_tools(
        &self,
        session_id: &str,
        cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        let res = self
            .send_request_with_context(
                session_id,
                None,
                None,
                ClientRequest::ListToolsRequest(RequestOptionalParam::with_param(
                    PaginatedRequestParams::default().with_cursor(cursor),
                )),
                cancel_token,
            )
            .await?;

        match res {
            ServerResult::ListToolsResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancel_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }
        let protocol_version = {
            let client = self.client.lock().await;
            client.peer_info().map(|info| info.protocol_version.clone())
        };
        if protocol_version.as_ref() == Some(&ProtocolVersion::V_2026_07_28) {
            let extensions = inject_session_context_into_extensions(
                Extensions::new(),
                Some(&ctx.session_id),
                ctx.working_dir_str(),
                ctx.tool_call_request_id.as_deref(),
            );
            if let Some(meta) = extensions.get::<MetaObject>() {
                params.meta.get_or_insert_default().0 .0.extend(
                    meta.0
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            let client = self.client.lock().await.clone();
            client.service().set_session_id(&ctx.session_id).await;
            let _active_tool_call_guard = ctx
                .tool_call_request_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|tool_call_request_id| {
                    client
                        .service()
                        .register_active_tool_call(&ctx.session_id, tool_call_request_id)
                });
            return tokio::select! {
                result = client.call_tool(params) => result,
                _ = tokio::time::sleep(self.timeout) => {
                    Err(ServiceError::Timeout { timeout: self.timeout })
                }
                _ = cancel_token.cancelled() => {
                    Err(ServiceError::Cancelled { reason: None })
                }
            };
        }

        let request = ClientRequest::CallToolRequest(Request::new(params));

        let result = self
            .send_request_with_context(
                &ctx.session_id,
                ctx.working_dir_str(),
                ctx.tool_call_request_id.as_deref(),
                request,
                cancel_token,
            )
            .await;

        match result? {
            ServerResult::CallToolResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn list_prompts(
        &self,
        session_id: &str,
        cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<ListPromptsResult, Error> {
        let res = self
            .send_request_with_context(
                session_id,
                None,
                None,
                ClientRequest::ListPromptsRequest(RequestOptionalParam::with_param(
                    PaginatedRequestParams::default().with_cursor(cursor),
                )),
                cancel_token,
            )
            .await?;

        match res {
            ServerResult::ListPromptsResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn get_prompt(
        &self,
        session_id: &str,
        name: &str,
        arguments: Value,
        cancel_token: CancellationToken,
    ) -> Result<GetPromptResult, Error> {
        let arguments = match arguments {
            Value::Object(map) => Some(map),
            _ => None,
        };
        let mut params = GetPromptRequestParams::new(name.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }
        let client = self.client.lock().await.clone();
        if client
            .peer_info()
            .is_some_and(|info| info.protocol_version == ProtocolVersion::V_2026_07_28)
        {
            client.service().set_session_id(session_id).await;
            return tokio::select! {
                result = client.get_prompt(params) => result,
                _ = tokio::time::sleep(self.timeout) => Err(ServiceError::Timeout { timeout: self.timeout }),
                _ = cancel_token.cancelled() => Err(ServiceError::Cancelled { reason: None }),
            };
        }
        drop(client);

        let res = self
            .send_request_with_context(
                session_id,
                None,
                None,
                ClientRequest::GetPromptRequest(Request::new(params)),
                cancel_token,
            )
            .await?;

        match res {
            ServerResult::GetPromptResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
        let (tx, rx) = mpsc::channel(16);
        self.notification_subscribers.lock().await.push(tx);
        rx
    }

    async fn update_working_dir(&self, new_dir: PathBuf) -> Result<(), Error> {
        self.do_update_working_dir(new_dir).await
    }
}

/// Injects the given session_id and working_dir into Extensions._meta.
/// None (or empty) removes any existing values.
fn inject_session_context_into_extensions(
    mut extensions: Extensions,
    session_id: Option<&str>,
    working_dir: Option<&str>,
    tool_call_request_id: Option<&str>,
) -> Extensions {
    let session_id = session_id.filter(|id| !id.is_empty());
    let working_dir = working_dir.filter(|dir| !dir.is_empty());
    let tool_call_request_id = tool_call_request_id.filter(|id| !id.is_empty());
    let mut meta_map = extensions
        .get::<MetaObject>()
        .map(|meta| meta.0.clone())
        .unwrap_or_default();

    // JsonObject is case-sensitive, so we use retain for case-insensitive removal
    meta_map.retain(|k, _| {
        !k.eq_ignore_ascii_case(SESSION_ID_HEADER)
            && !k.eq_ignore_ascii_case(WORKING_DIR_HEADER)
            && !k.eq_ignore_ascii_case(TOOL_CALL_REQUEST_ID_HEADER)
    });

    if let Some(session_id) = session_id {
        meta_map.insert(
            SESSION_ID_HEADER.to_string(),
            Value::String(session_id.to_string()),
        );
    }

    if let Some(working_dir) = working_dir {
        meta_map.insert(
            WORKING_DIR_HEADER.to_string(),
            Value::String(working_dir.to_string()),
        );
    }

    if let Some(tool_call_request_id) = tool_call_request_id {
        meta_map.insert(
            TOOL_CALL_REQUEST_ID_HEADER.to_string(),
            Value::String(tool_call_request_id.to_string()),
        );
    }

    extensions.insert(MetaObject(meta_map));
    extensions
}

fn inject_session_context_into_request(
    request: ClientRequest,
    session_id: Option<&str>,
    working_dir: Option<&str>,
    tool_call_request_id: Option<&str>,
) -> ClientRequest {
    match request {
        ClientRequest::ListResourcesRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                None,
            );
            ClientRequest::ListResourcesRequest(req)
        }
        ClientRequest::ReadResourceRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                None,
            );
            ClientRequest::ReadResourceRequest(req)
        }
        ClientRequest::ListToolsRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                None,
            );
            ClientRequest::ListToolsRequest(req)
        }
        ClientRequest::CallToolRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                tool_call_request_id,
            );
            ClientRequest::CallToolRequest(req)
        }
        ClientRequest::ListPromptsRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                None,
            );
            ClientRequest::ListPromptsRequest(req)
        }
        ClientRequest::GetPromptRequest(mut req) => {
            req.extensions = inject_session_context_into_extensions(
                req.extensions,
                session_id,
                working_dir,
                None,
            );
            ClientRequest::GetPromptRequest(req)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_text_preserves_text_first_provider_responses() {
        let response = crate::conversation::message::Message::assistant().with_text("answer");

        assert_eq!(
            extract_sampling_text(&response.content).as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn sampling_text_skips_thinking_before_final_text() {
        let response = crate::conversation::message::Message::assistant()
            .with_thinking("internal reasoning", "signature")
            .with_text("final answer");

        assert_eq!(
            extract_sampling_text(&response.content).as_deref(),
            Some("final answer")
        );
    }

    #[test]
    fn sampling_text_preserves_fragments_around_thinking() {
        let response = crate::conversation::message::Message::assistant()
            .with_text("first")
            .with_thinking("internal reasoning", "signature")
            .with_text(" second");

        assert_eq!(
            extract_sampling_text(&response.content).as_deref(),
            Some("first second")
        );
    }

    #[test]
    fn sampling_text_excludes_assistant_only_blocks_without_changing_visible_text() {
        let assistant_only = rmcp::model::TextContent::new("assistant only").with_annotations(
            rmcp::model::Annotations::default().with_audience(vec![Role::Assistant]),
        );
        let response = crate::conversation::message::Message::assistant()
            .with_text("Hello")
            .with_content(crate::conversation::message::MessageContent::Text(
                assistant_only,
            ))
            .with_text(" world");

        assert_eq!(
            extract_sampling_text(&response.content).as_deref(),
            Some("Hello world")
        );
    }

    #[test]
    fn sampling_text_rejects_thinking_only_responses() {
        let response = crate::conversation::message::Message::assistant()
            .with_thinking("internal reasoning", "signature");

        assert_eq!(extract_sampling_text(&response.content), None);
    }

    #[test]
    fn sampling_does_not_expose_json_from_thinking_only_response() {
        let response = crate::conversation::message::Message::assistant()
            .with_thinking("{\"private\":true}", "signature");

        assert_eq!(extract_sampling_text(&response.content), None);
    }
    use crate::agents::extension::ExtensionConfig;
    use crate::agents::GoosePlatform;
    use rmcp::model::Tool;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_case::test_case;
    use tokio::sync::Semaphore;

    struct BlockingToolsClient {
        calls: AtomicUsize,
        first_fetch_started: Semaphore,
        release_first_fetch: Semaphore,
    }

    #[async_trait::async_trait]
    impl McpClientTrait for BlockingToolsClient {
        async fn list_tools(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancel_token: CancellationToken,
        ) -> Result<ListToolsResult, Error> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let name = if call == 0 { "old" } else { "new" };

            if call == 0 {
                self.first_fetch_started.add_permits(1);
                let _permit = self.release_first_fetch.acquire().await.unwrap();
            }

            Ok(ListToolsResult {
                tools: vec![Tool::new(
                    name,
                    format!("{name} tool list"),
                    Arc::new(JsonObject::new()),
                )],
                next_cursor: None,
                meta: None,
                ..Default::default()
            })
        }

        async fn call_tool(
            &self,
            _ctx: &ToolCallContext,
            _name: &str,
            _arguments: Option<JsonObject>,
            _cancel_token: CancellationToken,
        ) -> Result<CallToolResult, Error> {
            Ok(CallToolResult::success(vec![]))
        }

        fn get_info(&self) -> Option<&InitializeResult> {
            None
        }
    }

    fn new_client(platform: GoosePlatform) -> GooseClient {
        let capabilities = match platform {
            GoosePlatform::GooseDesktop => GooseMcpClientCapabilities {
                mcpui: true,
                host_info: None,
                elicitation_handler: None,
                protocol_version: None,
            },
            GoosePlatform::GooseCli => GooseMcpClientCapabilities {
                mcpui: false,
                host_info: None,
                elicitation_handler: None,
                protocol_version: None,
            },
        };

        GooseClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            platform.to_string(),
            capabilities,
            std::env::current_dir().unwrap_or_default(),
            Arc::new(ActionRequiredManager::new()),
            Weak::new(),
        )
    }

    #[tokio::test]
    async fn tool_list_changed_during_fetch_prevents_stale_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let extension_manager = Arc::new(ExtensionManager::new_without_provider(
            temp_dir.path().to_path_buf(),
        ));
        let tools_client = Arc::new(BlockingToolsClient {
            calls: AtomicUsize::new(0),
            first_fetch_started: Semaphore::new(0),
            release_first_fetch: Semaphore::new(0),
        });
        let config = ExtensionConfig::Builtin {
            name: "dynamic".to_string(),
            display_name: Some("dynamic".to_string()),
            description: "dynamic tools".to_string(),
            timeout: None,
            bundled: None,
            available_tools: vec![],
        };
        extension_manager
            .add_client("dynamic".to_string(), config, tools_client.clone(), None)
            .await;

        let goose_client = GooseClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            "goose-test".to_string(),
            GooseMcpClientCapabilities {
                mcpui: false,
                host_info: None,
                elicitation_handler: None,
                protocol_version: None,
            },
            temp_dir.path().to_path_buf(),
            Arc::new(ActionRequiredManager::new()),
            Arc::downgrade(&extension_manager),
        );

        let manager = extension_manager.clone();
        let first_fetch = tokio::spawn(async move {
            manager
                .get_prefixed_tools("test-session", None)
                .await
                .unwrap()
        });

        let _started = tools_client.first_fetch_started.acquire().await.unwrap();
        goose_client.handle_tool_list_changed().await;
        tools_client.release_first_fetch.add_permits(1);

        let stale_result = first_fetch.await.unwrap();
        assert!(stale_result.iter().any(|tool| tool.name == "dynamic__old"));

        let refreshed = extension_manager
            .get_prefixed_tools("test-session", None)
            .await
            .unwrap();
        assert!(refreshed.iter().any(|tool| tool.name == "dynamic__new"));
        assert_eq!(tools_client.calls.load(Ordering::SeqCst), 2);
    }

    fn request_extensions(request: &ClientRequest) -> Option<&Extensions> {
        match request {
            ClientRequest::ListResourcesRequest(req) => Some(&req.extensions),
            ClientRequest::ReadResourceRequest(req) => Some(&req.extensions),
            ClientRequest::ListToolsRequest(req) => Some(&req.extensions),
            ClientRequest::CallToolRequest(req) => Some(&req.extensions),
            ClientRequest::ListPromptsRequest(req) => Some(&req.extensions),
            ClientRequest::GetPromptRequest(req) => Some(&req.extensions),
            _ => None,
        }
    }

    fn list_resources_request(extensions: Extensions) -> ClientRequest {
        let mut req = RequestOptionalParam::with_param(PaginatedRequestParams::default());
        req.extensions = extensions;
        ClientRequest::ListResourcesRequest(req)
    }

    fn read_resource_request(extensions: Extensions) -> ClientRequest {
        let mut req = Request::new(ReadResourceRequestParams::new(
            "test://resource".to_string(),
        ));
        req.extensions = extensions;
        ClientRequest::ReadResourceRequest(req)
    }

    fn list_tools_request(extensions: Extensions) -> ClientRequest {
        let mut req = RequestOptionalParam::with_param(PaginatedRequestParams::default());
        req.extensions = extensions;
        ClientRequest::ListToolsRequest(req)
    }

    fn call_tool_request(extensions: Extensions) -> ClientRequest {
        let mut req = Request::new(CallToolRequestParams::new("tool".to_string()));
        req.extensions = extensions;
        ClientRequest::CallToolRequest(req)
    }

    fn list_prompts_request(extensions: Extensions) -> ClientRequest {
        let mut req = RequestOptionalParam::with_param(PaginatedRequestParams::default());
        req.extensions = extensions;
        ClientRequest::ListPromptsRequest(req)
    }

    fn get_prompt_request(extensions: Extensions) -> ClientRequest {
        let mut req = Request::new(GetPromptRequestParams::new("prompt".to_string()));
        req.extensions = extensions;
        ClientRequest::GetPromptRequest(req)
    }

    #[test_case(
        Some("ext-session"),
        Some("current-session"),
        Some("ext-session");
        "extensions win"
    )]
    #[test_case(
        None,
        Some("current-session"),
        Some("current-session");
        "current when no extensions"
    )]
    #[test_case(
        None,
        None,
        None;
        "no session when no extensions or current"
    )]
    fn test_resolve_session_id(
        ext_session: Option<&str>,
        current_session: Option<&str>,
        expected: Option<&str>,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let client = new_client(GoosePlatform::GooseCli);
            if let Some(session_id) = current_session {
                client.set_session_id(session_id).await;
            }

            let extensions =
                inject_session_context_into_extensions(Extensions::new(), ext_session, None, None);

            let resolved = client.resolve_session_id(&extensions).await;

            let expected = expected.map(str::to_string);
            assert_eq!(resolved, expected);
        });
    }

    #[test]
    fn test_resolve_tool_call_request_id_from_extensions() {
        let client = new_client(GoosePlatform::GooseCli);
        let _guard = client.register_active_tool_call("session-a", "active-tool-call");
        let extensions = inject_session_context_into_extensions(
            Extensions::new(),
            Some("session-a"),
            None,
            Some("extension-tool-call"),
        );

        let resolved = client
            .resolve_tool_call_request_id("session-a", &extensions)
            .unwrap();

        assert_eq!(resolved, "extension-tool-call");
    }

    #[test]
    fn test_resolve_tool_call_request_id_from_active_call() {
        let client = new_client(GoosePlatform::GooseCli);
        let _guard = client.register_active_tool_call("session-a", "active-tool-call");

        let resolved = client
            .resolve_tool_call_request_id("session-a", &Extensions::new())
            .unwrap();

        assert_eq!(resolved, "active-tool-call");
    }

    #[test]
    fn test_resolve_tool_call_request_id_errors_when_calls_overlap() {
        let client = new_client(GoosePlatform::GooseCli);
        let _guard_a = client.register_active_tool_call("session-a", "active-tool-call-a");
        let _guard_b = client.register_active_tool_call("session-a", "active-tool-call-b");

        let error = client
            .resolve_tool_call_request_id("session-a", &Extensions::new())
            .expect_err("ambiguous elicitation should not resolve to an arbitrary call");

        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn test_resolve_tool_call_request_id_prefers_echoed_id_while_calls_overlap() {
        let client = new_client(GoosePlatform::GooseCli);
        let _guard_a = client.register_active_tool_call("session-a", "active-tool-call-a");
        let _guard_b = client.register_active_tool_call("session-a", "active-tool-call-b");
        let extensions = inject_session_context_into_extensions(
            Extensions::new(),
            Some("session-a"),
            None,
            Some("active-tool-call-a"),
        );

        let resolved = client
            .resolve_tool_call_request_id("session-a", &extensions)
            .unwrap();

        assert_eq!(resolved, "active-tool-call-a");
    }

    #[test]
    fn test_dropping_guard_unregisters_active_tool_call() {
        let client = new_client(GoosePlatform::GooseCli);
        let guard_a = client.register_active_tool_call("session-a", "active-tool-call-a");
        let _guard_b = client.register_active_tool_call("session-a", "active-tool-call-b");

        drop(guard_a);

        let resolved = client
            .resolve_tool_call_request_id("session-a", &Extensions::new())
            .unwrap();

        assert_eq!(resolved, "active-tool-call-b");
    }

    #[test_case(list_resources_request; "list_resources")]
    #[test_case(read_resource_request; "read_resource")]
    #[test_case(list_tools_request; "list_tools")]
    #[test_case(call_tool_request; "call_tool")]
    #[test_case(list_prompts_request; "list_prompts")]
    #[test_case(get_prompt_request; "get_prompt")]
    fn test_request_injects_session(request_builder: fn(Extensions) -> ClientRequest) {
        let session_id = "test-session-id";
        let mut extensions = Extensions::new();
        extensions.insert(
            serde_json::from_value::<MetaObject>(json!({
                "Goose-Session-Id": "old-session-id",
                "other-key": "preserve-me"
            }))
            .unwrap(),
        );

        let request = request_builder(extensions);
        let request = inject_session_context_into_request(request, Some(session_id), None, None);
        let extensions = request_extensions(&request).expect("request should have extensions");
        let meta = extensions
            .get::<MetaObject>()
            .expect("extensions should contain meta");

        assert_eq!(
            meta.0.get(SESSION_ID_HEADER),
            Some(&Value::String(session_id.to_string()))
        );
        assert_eq!(
            meta.0.get("other-key"),
            Some(&Value::String("preserve-me".to_string()))
        );
        if matches!(request, ClientRequest::CallToolRequest(_)) {
            assert!(!meta.0.contains_key(TOOL_CALL_REQUEST_ID_HEADER));
        }
    }

    #[test]
    fn test_session_id_in_mcp_meta() {
        let session_id = "test-session-789";
        let extensions = inject_session_context_into_extensions(
            Default::default(),
            Some(session_id),
            None,
            None,
        );
        let mcp_meta = extensions.get::<MetaObject>().unwrap();

        assert_eq!(
            &mcp_meta.0,
            json!({
                SESSION_ID_HEADER: session_id
            })
            .as_object()
            .unwrap()
        );
    }

    #[test_case(
        Some("new-session-id"),
        json!({
            SESSION_ID_HEADER: "new-session-id",
            "other-key": "preserve-me"
        });
        "replace"
    )]
    #[test_case(
        None,
        json!({
            "other-key": "preserve-me"
        });
        "remove"
    )]
    #[test_case(
        Some(""),
        json!({
            "other-key": "preserve-me"
        });
        "empty removes"
    )]
    fn test_session_id_case_insensitive_replacement(
        session_id: Option<&str>,
        expected_meta: serde_json::Value,
    ) {
        use rmcp::model::Extensions;
        use serde_json::from_value;

        let mut extensions = Extensions::new();
        extensions.insert(
            from_value::<MetaObject>(json!({
                SESSION_ID_HEADER: "old-session-1",
                "Agent-Session-Id": "old-session-2",
                "other-key": "preserve-me"
            }))
            .unwrap(),
        );

        let extensions = inject_session_context_into_extensions(extensions, session_id, None, None);
        let mcp_meta = extensions.get::<MetaObject>().unwrap();

        assert_eq!(&mcp_meta.0, expected_meta.as_object().unwrap());
    }

    #[test]
    fn test_tool_call_request_id_injected_only_for_call_tool() {
        let session_id = "test-session-id";
        let tool_call_request_id = "tool-request-1";

        let call_request = inject_session_context_into_request(
            call_tool_request(Extensions::new()),
            Some(session_id),
            None,
            Some(tool_call_request_id),
        );
        let call_meta = request_extensions(&call_request)
            .and_then(|extensions| extensions.get::<MetaObject>())
            .expect("call request should have meta");
        assert_eq!(
            call_meta.0.get(TOOL_CALL_REQUEST_ID_HEADER),
            Some(&Value::String(tool_call_request_id.to_string()))
        );

        let tools_request = inject_session_context_into_request(
            list_tools_request(Extensions::new()),
            Some(session_id),
            None,
            Some(tool_call_request_id),
        );
        let tools_meta = request_extensions(&tools_request)
            .and_then(|extensions| extensions.get::<MetaObject>())
            .expect("list tools request should have meta");
        assert!(!tools_meta.0.contains_key(TOOL_CALL_REQUEST_ID_HEADER));
    }

    #[test]
    fn test_client_info_advertises_mcp_apps_ui_extension() {
        let client = new_client(GoosePlatform::GooseDesktop);
        let info = ClientHandler::get_info(&client);

        // Verify the client advertises the MCP Apps UI extension capability
        let extensions = info
            .capabilities
            .extensions
            .expect("capabilities should have extensions");

        let ui_ext = extensions
            .get("io.modelcontextprotocol/ui")
            .expect("should have io.modelcontextprotocol/ui extension");

        let mime_types = ui_ext
            .get("mimeTypes")
            .expect("ui extension should have mimeTypes");

        assert_eq!(mime_types, &json!(["text/html;profile=mcp-app"]));
    }

    #[test]
    fn test_client_capabilities_advertise_roots() {
        let client = new_client(GoosePlatform::GooseCli);
        let info = ClientHandler::get_info(&client);
        assert!(
            info.capabilities.roots.is_some(),
            "client should advertise roots capability"
        );
    }

    #[test]
    fn test_explicit_host_info_passes_through_client_identity() {
        let client = GooseClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            GoosePlatform::GooseDesktop.to_string(),
            GooseMcpClientCapabilities {
                mcpui: true,
                host_info: Some(GooseMcpHostInfo {
                    explicit_extensions: true,
                    extensions: ExtensionCapabilities::new(),
                    client_name: Some("goose2".to_string()),
                    client_version: Some("0.1.0".to_string()),
                }),
                elicitation_handler: None,
                protocol_version: None,
            },
            std::env::current_dir().unwrap_or_default(),
            Arc::new(ActionRequiredManager::new()),
            Weak::new(),
        );

        let info = ClientHandler::get_info(&client);
        assert_eq!(info.client_info.name, "goose2");
        assert_eq!(info.client_info.version, "0.1.0");
        let extensions = info
            .capabilities
            .extensions
            .expect("client should still serialize an extensions object");
        assert!(
            !extensions.contains_key(MCP_APPS_UI_EXTENSION_ID),
            "explicit empty host extensions should disable platform fallback"
        );
    }

    #[test]
    fn test_explicit_host_extensions_override_platform_fallback() {
        let client = GooseClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            GoosePlatform::GooseCli.to_string(),
            GooseMcpClientCapabilities {
                mcpui: false,
                host_info: Some(GooseMcpHostInfo {
                    explicit_extensions: true,
                    extensions: default_mcp_apps_ui_extensions(),
                    client_name: Some("goose2".to_string()),
                    client_version: Some("0.1.0".to_string()),
                }),
                elicitation_handler: None,
                protocol_version: None,
            },
            std::env::current_dir().unwrap_or_default(),
            Arc::new(ActionRequiredManager::new()),
            Weak::new(),
        );

        let info = ClientHandler::get_info(&client);
        let extensions = info
            .capabilities
            .extensions
            .expect("capabilities should have explicit host extensions");

        assert!(extensions.contains_key(MCP_APPS_UI_EXTENSION_ID));
        assert_eq!(info.client_info.name, "goose2");
    }

    #[test]
    fn test_host_identity_does_not_disable_platform_fallback_without_explicit_extensions() {
        let client = GooseClient::new(
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(None)),
            GoosePlatform::GooseDesktop.to_string(),
            GooseMcpClientCapabilities {
                mcpui: true,
                host_info: Some(GooseMcpHostInfo {
                    explicit_extensions: false,
                    extensions: ExtensionCapabilities::new(),
                    client_name: Some("goose2".to_string()),
                    client_version: Some("0.1.0".to_string()),
                }),
                elicitation_handler: None,
                protocol_version: None,
            },
            std::env::current_dir().unwrap_or_default(),
            Arc::new(ActionRequiredManager::new()),
            Weak::new(),
        );

        let info = ClientHandler::get_info(&client);
        let extensions = info
            .capabilities
            .extensions
            .expect("platform fallback should still advertise MCP Apps UI");

        assert!(extensions.contains_key(MCP_APPS_UI_EXTENSION_ID));
        assert_eq!(info.client_info.name, "goose2");
    }

    #[test]
    #[expect(deprecated)]
    fn test_working_dir_roots_returns_current_dir_as_root() {
        let dir = PathBuf::from("/tmp/test-project");
        let result = working_dir_roots(&dir);
        assert_eq!(result.roots.len(), 1);
        assert_eq!(result.roots[0].uri, "file:///tmp/test-project");
        assert_eq!(result.roots[0].name.as_deref(), Some("working_directory"));
    }

    #[tokio::test]
    async fn fan_out_notification_prunes_closed_subscribers() {
        use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken};

        let handlers = Arc::new(Mutex::new(Vec::new()));
        let mut receivers = Vec::new();
        for _ in 0..5 {
            let (tx, rx) = mpsc::channel(16);
            handlers.lock().await.push(tx);
            receivers.push(rx);
        }
        assert_eq!(handlers.lock().await.len(), 5);

        // Drop all receivers, simulating tool-call completion.
        drop(receivers);

        let notification = ServerNotification::ProgressNotification(Notification::new(
            ProgressNotificationParam::new(
                ProgressToken(NumberOrString::String(Arc::from("token"))),
                1.0,
            ),
        ));
        fan_out_notification(&mut *handlers.lock().await, notification);
        assert!(
            handlers.lock().await.is_empty(),
            "closed subscribers must be pruned on fan-out"
        );

        // A live subscriber survives fan-out; subsequent closed ones still prune.
        let mut live_rx = {
            let (tx, rx) = mpsc::channel(16);
            handlers.lock().await.push(tx);
            rx
        };
        for _ in 0..3 {
            let (tx, rx) = mpsc::channel(16);
            handlers.lock().await.push(tx);
            drop(rx);
        }
        assert_eq!(handlers.lock().await.len(), 4);

        let notification = ServerNotification::ProgressNotification(Notification::new(
            ProgressNotificationParam::new(
                ProgressToken(NumberOrString::String(Arc::from("token-2"))),
                2.0,
            ),
        ));
        fan_out_notification(&mut *handlers.lock().await, notification.clone());
        assert_eq!(handlers.lock().await.len(), 1);
        let received = live_rx
            .recv()
            .await
            .expect("live subscriber should receive");
        assert!(matches!(
            received,
            ServerNotification::ProgressNotification(_)
        ));
    }
}
