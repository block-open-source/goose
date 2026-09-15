use agent_client_protocol::schema::v1::{
    Annotations as AcpAnnotations, ClientCapabilities, CloseSessionRequest, ContentBlock,
    ContentChunk, EnvVariable, HttpHeader, ImageContent, InitializeRequest, InitializeResponse,
    LoadSessionRequest, McpCapabilities, McpServer, McpServerHttp, McpServerStdio,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, Role as AcpRole, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionConfigSelectOptions, SessionId, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, SetSessionModeResponse, StopReason,
    TextContent, ToolCallContent, ToolCallStatus, ToolKind,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectionTo};
use agent_client_protocol_schema::v1::Usage as AcpUsage;
use agent_client_protocol_schema::v1::AGENT_METHOD_NAMES;
use anyhow::{Context, Result};
use async_stream::try_stream;
use futures::future::BoxFuture;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock as RmcpContent, Role, Tool};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch, Mutex as TokioMutex};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use crate::acp::handoff::{build_handoff_context_memo, memo_token_budget, prompt_token_cost};
use crate::acp::{map_permission_response, PermissionDecision};
use crate::config::{Config, ExtensionConfig, GooseMode};
use crate::conversation::message::{Message, MessageContent, TOOL_META_EXTERNAL_DISPATCH_KEY};
use crate::permission::permission_confirmation::PrincipalType;
use crate::permission::{Permission, PermissionConfirmation};
use crate::providers::base::{MessageStream, PermissionRouting, Provider};
use crate::subprocess::configure_subprocess;
use crate::token_counter::create_token_counter;
use crate::utils::sanitize_unicode_tags;
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use goose_providers::thinking::{
    ThinkingEffortCapability, ThinkingEffortOption, ThinkingEffortSupport,
};

/// Sentinel: resolved to the actual model name during connect().
pub const ACP_CURRENT_MODEL: &str = "current";

/// Config option id used by agents that advertise a thinking-effort selector
/// without categorizing it as `thought_level`.
const EFFORT_CONFIG_OPTION_ID: &str = "effort";

/// Session request param holding the selected thinking effort.
pub(super) const THINKING_EFFORT_PARAM: &str = "thinking_effort";

pub struct AcpProviderConfig {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub work_dir: PathBuf,
    pub mcp_servers: Vec<McpServer>,
    pub session_mode_id: Option<String>,
    pub session_config_options: Vec<(String, String)>,
    /// Config option id used to select the model (e.g. `"model"`). When set, the
    /// provider re-applies this option from the per-completion `ModelConfig`
    /// whenever the active session model changes.
    pub model_config_option_id: Option<String>,
    pub mode_mapping: HashMap<GooseMode, Vec<String>>,
    pub notification_callback: Option<Arc<dyn Fn(SessionNotification) + Send + Sync>>,
}

enum ClientRequest {
    NewSession {
        response_tx: oneshot::Sender<Result<NewSessionResponse>>,
    },
    LoadSession {
        session_id: SessionId,
        response_tx: oneshot::Sender<Result<NewSessionResponse>>,
    },
    CloseSession {
        session_id: SessionId,
    },
    SetMode {
        session_id: SessionId,
        mode_id: String,
        response_tx: oneshot::Sender<Result<()>>,
    },
    SetConfigOption {
        session_id: SessionId,
        config_id: String,
        value: String,
        response_tx: oneshot::Sender<Result<()>>,
    },
    Prompt {
        session_id: SessionId,
        content: Vec<ContentBlock>,
        response_tx: mpsc::Sender<AcpUpdate>,
    },
}

// tokio I/O handles can't move between runtimes, so the child process must be
// spawned inside the OS thread. This closure lets start() share all other logic.
type ClientLoopFn = Box<
    dyn FnOnce(
            AcpClientLoop,
            mpsc::Receiver<ClientRequest>,
            oneshot::Sender<Result<InitializeResponse>>,
            oneshot::Receiver<()>,
        ) -> BoxFuture<'static, ()>
        + Send,
>;

struct ClientLoopGuard {
    tx: Option<mpsc::Sender<ClientRequest>>,
    cancel_tx: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ClientLoopGuard {
    fn drop(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            std::thread::spawn(move || {
                if let Err(error) = thread.join() {
                    tracing::debug!("AcpClientLoop thread panicked: {error:?}");
                }
            });
        }
    }
}

#[derive(Debug)]
enum AcpUpdate {
    Text(TextContent),
    Thought(String),
    ToolCallStart {
        id: String,
        name: String,
        kind: ToolKind,
        raw_input: Option<serde_json::Value>,
    },
    ToolCallComplete {
        id: String,
        raw_output: Option<serde_json::Value>,
        content: Option<Vec<ToolCallContent>>,
        is_error: bool,
    },
    PermissionRequest {
        request: Box<RequestPermissionRequest>,
        response_tx: oneshot::Sender<RequestPermissionResponse>,
    },
    Complete(StopReason, Option<AcpUsage>),
    Error(agent_client_protocol::Error),
}

/// Whether dropping the handoff memo could plausibly change the outcome. An agent that
/// rejected the very first update has told us nothing except that it disliked the prompt,
/// and the memo is the only part we added — but a spent account or a missing credential
/// says nothing about the prompt at all, so retrying would burn the single fallback the
/// session gets and consume a memo the agent never actually refused.
fn retry_without_memo_could_help(error: &agent_client_protocol::Error) -> bool {
    if error.code == agent_client_protocol::schema::v1::ErrorCode::AuthRequired {
        return false;
    }
    error
        .data
        .as_ref()
        .and_then(|data| data.get("reason"))
        .and_then(serde_json::Value::as_str)
        != Some(crate::acp::CREDITS_EXHAUSTED_REASON)
}

fn provider_error_from_acp(error: agent_client_protocol::Error) -> ProviderError {
    if error.code == agent_client_protocol::schema::v1::ErrorCode::AuthRequired {
        ProviderError::Authentication(error.to_string())
    } else {
        ProviderError::RequestFailed(error.to_string())
    }
}

/// Per-tool-call buffer for accumulating ACP ToolCallUpdate fields across
/// non-terminal updates, drained on the terminal status update.
#[derive(Debug, Default)]
struct AccumulatedToolCall {
    raw_output: Option<serde_json::Value>,
    content: Vec<ToolCallContent>,
}

/// The single ACP session backing this provider instance.
#[derive(Clone)]
struct AcpSession {
    id: SessionId,
    response: NewSessionResponse,
}

struct HandoffContextClaim {
    first_prompt: bool,
    include_context: bool,
}

struct HandoffContextClaimGuard {
    handoff_context_sent: Arc<AtomicBool>,
    pending: bool,
}

impl HandoffContextClaimGuard {
    fn new(handoff_context_sent: Arc<AtomicBool>, first_prompt: bool) -> Self {
        Self {
            handoff_context_sent,
            pending: first_prompt,
        }
    }

    fn commit(&mut self) {
        self.pending = false;
    }

    fn rollback(&mut self) {
        if self.pending {
            self.handoff_context_sent.store(false, Ordering::Release);
            self.pending = false;
        }
    }
}

impl Drop for HandoffContextClaimGuard {
    fn drop(&mut self) {
        if self.pending {
            self.handoff_context_sent.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone)]
struct AcpEffortState {
    capability: Arc<Mutex<Option<ThinkingEffortCapability>>>,
    updates: watch::Sender<ThinkingEffortSupport>,
}

impl AcpEffortState {
    fn new() -> Self {
        let (updates, _) = watch::channel(ThinkingEffortSupport::Unsupported);
        Self {
            capability: Arc::new(Mutex::new(None)),
            updates,
        }
    }
}

#[derive(Clone)]
struct AcpSessionState {
    active_id: Arc<Mutex<Option<SessionId>>>,
    effort: AcpEffortState,
}

impl AcpSessionState {
    fn new(effort: AcpEffortState) -> Self {
        Self {
            active_id: Arc::new(Mutex::new(None)),
            effort,
        }
    }
}

pub struct AcpProvider {
    name: String,
    goose_mode: Arc<Mutex<GooseMode>>,
    mode_mapping: HashMap<GooseMode, Vec<String>>,

    session: Mutex<AcpSession>,

    pending_confirmations:
        Arc<TokioMutex<HashMap<String, oneshot::Sender<PermissionConfirmation>>>>,
    pending_tool_updates: Arc<Mutex<HashMap<String, AccumulatedToolCall>>>,
    /// True after the first ACP prompt completes with the handoff context committed.
    /// Failed or abandoned first prompts reset this so the next prompt can retry it.
    handoff_context_sent: Arc<AtomicBool>,
    /// Latest `size` reported by the ACP server in a `session/update` →
    /// `usage_update` notification. 0 means no real update has arrived yet.
    context_size: Arc<AtomicU64>,

    /// Config option id used to select the model, if this agent supports it.
    model_config_option_id: Option<String>,
    /// Model currently applied via `model_config_option_id`, used to avoid
    /// redundant `SetConfigOption` calls.
    applied_model: Arc<Mutex<Option<String>>>,

    /// The agent's thinking-effort config option, mirrored from every
    /// config-options payload it sends. `None` means the agent offers no effort
    /// selector for its current model. Its `current` value doubles as the
    /// redundant-send guard: unlike a goose-side cache of the last applied
    /// value, it tracks the agent resetting its own effort (e.g. on a model
    /// switch), so the persisted value is re-applied when that happens.
    effort: AcpEffortState,

    tx: Option<mpsc::Sender<ClientRequest>>,
    cancel_tx: Option<oneshot::Sender<()>>,
    loop_thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for AcpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpProvider")
            .field("name", &self.name)
            .finish()
    }
}

fn spawn_client_loop(fut: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build ACP client runtime");
        rt.block_on(fut)
    })
}

impl AcpProvider {
    pub async fn connect(
        name: String,
        goose_mode: GooseMode,
        config: AcpProviderConfig,
    ) -> Result<Self> {
        Self::start(
            name,
            goose_mode,
            config,
            Box::new(|cl, rx, init_tx, mut cancel_rx| {
                Box::pin(async move {
                    tokio::select! {
                        biased;
                        _ = &mut cancel_rx => {}
                        _ = cl.spawn(rx, init_tx) => {}
                    }
                })
            }),
        )
        .await
    }

    #[doc(hidden)]
    pub async fn connect_with_transport(
        name: String,
        goose_mode: GooseMode,
        config: AcpProviderConfig,
        transport: impl agent_client_protocol::ConnectTo<Client> + 'static,
    ) -> Result<Self> {
        Self::start(
            name,
            goose_mode,
            config,
            Box::new(move |cl, mut rx, init_tx, mut cancel_rx| {
                Box::pin(async move {
                    tokio::select! {
                        biased;
                        _ = &mut cancel_rx => {}
                        result = cl.run(transport, &mut rx, init_tx) => {
                            if let Err(e) = result {
                                tracing::error!("ACP protocol error: {e}");
                            }
                        }
                    }
                })
            }),
        )
        .await
    }

    async fn start(
        name: String,
        goose_mode: GooseMode,
        config: AcpProviderConfig,
        run: ClientLoopFn,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel(32);
        let (init_tx, init_rx) = oneshot::channel();
        let mode_mapping = config.mode_mapping.clone();
        let model_config_option_id = config.model_config_option_id.clone();
        let applied_model = config.model_config_option_id.as_ref().and_then(|id| {
            config
                .session_config_options
                .iter()
                .find(|(opt_id, _)| opt_id == id)
                .map(|(_, value)| value.clone())
        });
        let goose_mode_shared = Arc::new(Mutex::new(goose_mode));
        let pending_tool_updates: Arc<Mutex<HashMap<String, AccumulatedToolCall>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let context_size = Arc::new(AtomicU64::new(0));
        let effort = AcpEffortState::new();
        let client_loop = AcpClientLoop::new(
            config,
            goose_mode_shared.clone(),
            pending_tool_updates.clone(),
            context_size.clone(),
            effort.clone(),
        );
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let loop_thread = spawn_client_loop(run(client_loop, rx, init_tx, cancel_rx));
        let mut client_loop_guard = ClientLoopGuard {
            tx: Some(tx),
            cancel_tx: Some(cancel_tx),
            thread: Some(loop_thread),
        };

        let _init_response = init_rx
            .await
            .context("ACP client initialization cancelled")??;

        // Create the ACP session eagerly during connect.
        let (session_tx, session_rx) = oneshot::channel();
        client_loop_guard
            .tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::NewSession {
                response_tx: session_tx,
            })
            .await
            .context("ACP client is unavailable")?;
        let response = session_rx
            .await
            .context("ACP session creation cancelled")??;

        let session = AcpSession {
            id: response.session_id.clone(),
            response,
        };

        Ok(Self {
            name,
            goose_mode: goose_mode_shared,
            mode_mapping,
            session: Mutex::new(session),
            pending_confirmations: Arc::new(TokioMutex::new(HashMap::new())),
            pending_tool_updates,
            handoff_context_sent: Arc::new(AtomicBool::new(false)),
            context_size,
            model_config_option_id,
            applied_model: Arc::new(Mutex::new(applied_model)),
            effort,
            tx: client_loop_guard.tx.take(),
            cancel_tx: client_loop_guard.cancel_tx.take(),
            loop_thread: client_loop_guard.thread.take(),
        })
    }

    fn acp_session_id(&self) -> SessionId {
        self.session.lock().unwrap().id.clone()
    }

    async fn load_session(&self, session_id: SessionId) -> Result<AcpSession> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::LoadSession {
                session_id,
                response_tx,
            })
            .await
            .context("ACP client is unavailable")?;
        let response = response_rx.await.context("ACP session load cancelled")??;
        Ok(AcpSession {
            id: response.session_id.clone(),
            response,
        })
    }

    pub(crate) async fn send_set_mode(&self, _goose_id: &str, mode_id: String) -> Result<()> {
        let session_id = self.acp_session_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::SetMode {
                session_id,
                mode_id,
                response_tx,
            })
            .await
            .context("ACP client is unavailable")?;
        response_rx.await.context("ACP request cancelled")?
    }

    pub(crate) async fn send_set_config_option(
        &self,
        _goose_id: &str,
        config_id: String,
        value: String,
    ) -> Result<()> {
        let session_id = self.acp_session_id();
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::SetConfigOption {
                session_id,
                config_id,
                value,
                response_tx,
            })
            .await
            .context("ACP client is unavailable")?;
        response_rx.await.context("ACP request cancelled")?
    }

    /// Re-apply the model selection config option when the active session model
    /// differs from what was last applied. ACP agents that select their model
    /// via a config option (e.g. Copilot) need this so resumed or switched
    /// sessions actually use the requested model instead of the agent default.
    async fn apply_model_if_changed(&self, model_name: &str) -> Result<()> {
        let Some(config_id) = self.model_config_option_id.clone() else {
            return Ok(());
        };
        if model_name == ACP_CURRENT_MODEL {
            return Ok(());
        }

        {
            let applied = self
                .applied_model
                .lock()
                .map_err(|_| anyhow::anyhow!("applied_model lock poisoned"))?;
            if applied.as_deref() == Some(model_name) {
                return Ok(());
            }
        }

        self.send_set_config_option("", config_id, model_name.to_string())
            .await?;

        let mut applied = self
            .applied_model
            .lock()
            .map_err(|_| anyhow::anyhow!("applied_model lock poisoned"))?;
        *applied = Some(model_name.to_string());
        Ok(())
    }

    fn effort_capability(&self) -> Result<Option<ThinkingEffortCapability>> {
        Ok(self
            .effort
            .capability
            .lock()
            .map_err(|_| anyhow::anyhow!("effort_state lock poisoned"))?
            .clone())
    }

    /// The redundant-send guard is the mirrored capability's `current`, not a
    /// goose-side record of the last sent value: the agent's `SetConfigOption`
    /// response refreshes `effort_state` before this call returns, and later
    /// refreshes track the agent resetting its own effort.
    async fn set_effort_option(
        &self,
        goose_id: &str,
        option_id: &str,
        value: String,
    ) -> Result<()> {
        {
            let state = self
                .effort
                .capability
                .lock()
                .map_err(|_| anyhow::anyhow!("effort_state lock poisoned"))?;
            let current = state
                .as_ref()
                .and_then(|capability| capability.current.as_deref());
            if current == Some(value.as_str()) {
                return Ok(());
            }
        }

        self.send_set_config_option(goose_id, option_id.to_string(), value)
            .await
    }

    /// Forward the session's thinking effort to the agent when it differs from
    /// the agent's mirrored current value. A recreated provider (model switch,
    /// provider switch, session reload) starts from the agent's own default, so
    /// the value has to be re-applied rather than assumed. It comes from
    /// `resolve_effort_value`, the same resolver the config menu advertises
    /// from, so the selection ACP clients see is the one that gets sent.
    async fn apply_effort_if_changed(&self, model_config: &ModelConfig) -> Result<()> {
        let Some(capability) = self.effort_capability()? else {
            return Ok(());
        };
        let Some(mapped) = resolve_effort_value(&capability, model_config) else {
            return Ok(());
        };

        self.set_effort_option("", &capability.option_id, mapped)
            .await
    }

    async fn prompt(
        &self,
        session_id: SessionId,
        content: Vec<ContentBlock>,
    ) -> Result<mpsc::Receiver<AcpUpdate>> {
        let (response_tx, response_rx) = mpsc::channel(64);
        self.tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::Prompt {
                session_id,
                content,
                response_tx,
            })
            .await
            .context("ACP client is unavailable")?;
        Ok(response_rx)
    }

    fn session_has_config_option(&self, category: SessionConfigOptionCategory) -> bool {
        self.session
            .lock()
            .unwrap()
            .response
            .config_options
            .as_ref()
            .is_some_and(|opts| opts.iter().any(|o| o.category.as_ref() == Some(&category)))
    }

    fn claim_handoff_context(&self, messages: &[Message]) -> HandoffContextClaim {
        let first_prompt = !self.handoff_context_sent.swap(true, Ordering::AcqRel);
        HandoffContextClaim {
            first_prompt,
            include_context: first_prompt && has_handoff_context(messages),
        }
    }

    /// Prior conversation, bounded against the agent's context window. The agent's own
    /// system prompt and tool schemas are invisible to us, hence the conservative share.
    async fn bounded_handoff_memo(
        &self,
        model_config: &ModelConfig,
        messages: &[Message],
        current_prompt: &[ContentBlock],
    ) -> Option<String> {
        let last_user_index = last_user_message_index(messages)?;
        let counter = match create_token_counter().await {
            Ok(counter) => counter,
            Err(error) => {
                tracing::error!(%error, "no token counter, dropping ACP handoff context");
                return None;
            }
        };

        let context_limit = crate::context_limit::get_context_limit(self, &model_config.model_name)
            .await
            .ok()?;
        let budget = memo_token_budget(context_limit, prompt_token_cost(current_prompt, &counter));

        build_handoff_context_memo(&messages[..last_user_index], budget, &counter)
    }
}

fn fresh_text_run() -> (String, i64) {
    (
        uuid::Uuid::new_v4().to_string(),
        chrono::Utc::now().timestamp(),
    )
}

#[async_trait::async_trait]
impl Provider for AcpProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn provider_session_id(&self) -> Option<String> {
        Some(self.acp_session_id().to_string())
    }

    async fn resume(&self, session_id: &str) -> Result<(), ProviderError> {
        if self.acp_session_id().0.as_ref() == session_id {
            return Ok(());
        }

        let previous_session_id = self.acp_session_id();
        let loaded = self
            .load_session(SessionId::new(session_id))
            .await
            .map_err(|error| ProviderError::RequestFailed(error.to_string()))?;
        *self.session.lock().unwrap() = loaded;
        self.handoff_context_sent.store(true, Ordering::Release);
        let _ = self
            .tx
            .as_ref()
            .unwrap()
            .send(ClientRequest::CloseSession {
                session_id: previous_session_id,
            })
            .await;
        Ok(())
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        goose_providers::context_limit::ContextLimitResolver::new(self.get_name())
            .resolve(model, override_limit, || async {
                let size = self.context_size.load(Ordering::Relaxed);
                Ok((size > 0).then_some(size as usize))
            })
            .await
    }

    async fn update_mode(&self, session_id: &str, mode: GooseMode) -> Result<(), ProviderError> {
        if let Some(candidates) = self.mode_mapping.get(&mode) {
            let session = self.session.lock().unwrap().clone();
            let mode_str =
                select_mode_id(candidates, session.response.modes.as_ref()).ok_or_else(|| {
                    ProviderError::RequestFailed(format!(
                        "None of the mode ids [{}] are offered by the agent",
                        candidates.join(", ")
                    ))
                })?;
            if self.session_has_config_option(SessionConfigOptionCategory::Mode) {
                self.send_set_config_option(session_id, "mode".into(), mode_str)
                    .await
                    .map_err(|e| {
                        ProviderError::RequestFailed(format!("Failed to set mode: {e}"))
                    })?;
            } else {
                self.send_set_mode(session_id, mode_str)
                    .await
                    .map_err(|e| {
                        ProviderError::RequestFailed(format!("Failed to set mode: {e}"))
                    })?;
            }
        }

        if let Ok(mut guard) = self.goose_mode.lock() {
            *guard = mode;
        }
        Ok(())
    }

    fn thinking_effort_support(&self) -> ThinkingEffortSupport {
        match self.effort_capability().ok().flatten() {
            Some(capability) => ThinkingEffortSupport::Options(capability),
            None => ThinkingEffortSupport::Unsupported,
        }
    }

    fn subscribe_thinking_effort_support(&self) -> Option<watch::Receiver<ThinkingEffortSupport>> {
        Some(self.effort.updates.subscribe())
    }

    async fn set_thinking_effort(
        &self,
        session_id: &str,
        value: &str,
    ) -> Result<bool, ProviderError> {
        let capability = self
            .effort_capability()
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;
        // No effort selector: the only advertised choice is `off`, which is a
        // no-op. Falling back to the caller's legacy path would respawn the
        // agent, while accepting any other value would persist a setting that
        // was never applied.
        let Some(capability) = capability else {
            return if value.eq_ignore_ascii_case("off") {
                Ok(true)
            } else {
                Err(ProviderError::InvalidValue(format!(
                    "Agent offers no thinking effort '{value}'"
                )))
            };
        };
        let mapped = map_effort_value(&capability, value).ok_or_else(|| {
            ProviderError::InvalidValue(format!("Agent offers no thinking effort '{value}'"))
        })?;

        self.set_effort_option(session_id, &capability.option_id, mapped)
            .await
            .map_err(|e| effort_option_error(value, e))?;
        Ok(true)
    }

    async fn apply_model_selection(&self, model_config: &ModelConfig) -> Result<(), ProviderError> {
        self.apply_model_if_changed(&model_config.model_name)
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to set ACP model option: {e}"))
            })
    }

    fn permission_routing(&self) -> PermissionRouting {
        PermissionRouting::ActionRequired
    }

    fn manages_own_context(&self) -> bool {
        true
    }

    async fn handle_permission_confirmation(
        &self,
        request_id: &str,
        confirmation: &PermissionConfirmation,
    ) -> bool {
        let mut pending = self.pending_confirmations.lock().await;
        if let Some(tx) = pending.remove(request_id) {
            let _ = tx.send(confirmation.clone());
            return true;
        }
        false
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        _system: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let session_id = self.acp_session_id();

        self.apply_model_if_changed(&model_config.model_name)
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to set ACP model option: {e}"))
            })?;

        self.apply_effort_if_changed(model_config)
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to set ACP effort option: {e}"))
            })?;

        let current_prompt_blocks = messages_to_prompt(messages, None);
        if current_prompt_blocks.is_empty() {
            return Ok(Box::pin(futures::stream::empty()));
        }

        let claim = self.claim_handoff_context(messages);
        let mut handoff_claim_guard =
            HandoffContextClaimGuard::new(self.handoff_context_sent.clone(), claim.first_prompt);
        let memo = if claim.include_context {
            self.bounded_handoff_memo(model_config, messages, &current_prompt_blocks)
                .await
        } else {
            None
        };
        if claim.include_context && memo.is_none() {
            // Nothing fit beside this turn's prompt, so the context never left goose. Give
            // it back rather than marking a handoff that never happened as done — a single
            // oversized turn would otherwise cost the session its whole history.
            handoff_claim_guard.rollback();
        }
        // A memo is only ever an estimate of what the agent will accept, so keep the bare
        // prompt to retry with. Without it a bad estimate leaves the session unresumable.
        let (prompt_blocks, mut bare_retry_blocks) = match memo {
            Some(memo) => (
                messages_to_prompt(messages, Some(memo)),
                Some(current_prompt_blocks),
            ),
            None => (current_prompt_blocks, None),
        };
        // Drop any tool-call buffer state left over from a prior prompt
        // (e.g. cancelled or interrupted before its terminal status arrived).
        if let Ok(mut buffer) = self.pending_tool_updates.lock() {
            buffer.clear();
        }
        let mut rx = match self.prompt(session_id.clone(), prompt_blocks).await {
            Ok(rx) => rx,
            Err(e) => match bare_retry_blocks.take() {
                Some(blocks) => {
                    // Consume the handoff before retrying. The memo is the only thing this
                    // attempt added, so rebuilding it next time would reproduce the same
                    // rejection and leave the session permanently unresumable.
                    handoff_claim_guard.commit();
                    self.prompt(session_id.clone(), blocks)
                        .await
                        .map_err(|retry_error| {
                            ProviderError::RequestFailed(format!(
                                "Failed to send ACP prompt: {retry_error}"
                            ))
                        })?
                }
                // Nothing was added to this prompt, so the guard rolls the claim back as
                // it drops and the next attempt can still carry the context.
                None => {
                    return Err(ProviderError::RequestFailed(format!(
                        "Failed to send ACP prompt: {e}"
                    )));
                }
            },
        };
        let bare_retry =
            bare_retry_blocks.map(|blocks| (self.tx.as_ref().unwrap().clone(), session_id, blocks));

        let pending_confirmations = self.pending_confirmations.clone();
        let goose_mode = *self
            .goose_mode
            .lock()
            .map_err(|_| ProviderError::RequestFailed("goose_mode lock poisoned".into()))?;

        let reject_all_tools = goose_mode == GooseMode::Chat;
        let model_name = model_config.model_name.clone();

        Ok(Box::pin(try_stream! {
            let mut suppress_text = false;
            let mut bare_retry = bare_retry;
            let mut updates_seen = 0usize;
            let mut rejected_tool_calls: HashSet<String> = HashSet::new();
            // Stable id+timestamp per contiguous run so Desktop coalesces chunks into one bubble.
            let mut text_run: Option<(String, i64)> = None;
            let mut thought_run: Option<(String, i64)> = None;

            while let Some(update) = rx.recv().await {
                updates_seen += 1;
                match update {
                    AcpUpdate::Text(text) => {
                        if !suppress_text {
                            let (id, ts) = text_run
                                .get_or_insert_with(fresh_text_run)
                                .clone();
                            let message = acp_text_update_message(text, id, ts);
                            yield (Some(message), None);
                        }
                    }
                    AcpUpdate::Thought(text) => {
                        let (id, ts) = thought_run
                            .get_or_insert_with(fresh_text_run)
                            .clone();
                        let message = Message::new(Role::Assistant, ts, vec![])
                            .with_thinking(text, "")
                            .with_visibility(true, false)
                            .with_id(id);
                        yield (Some(message), None);
                    }
                    AcpUpdate::ToolCallStart { id, name, kind, raw_input } => {
                        text_run = None;
                        thought_run = None;
                        if reject_all_tools {
                            suppress_text = true;
                            rejected_tool_calls.insert(id);
                        } else {
                            let mut params = CallToolRequestParams::new(name);
                            if let Some(serde_json::Value::Object(map)) = raw_input {
                                params = params.with_arguments(map);
                            }
                            // external_dispatch tells the agent loop not to redispatch this
                            // call. goose.acp.kind preserves ACP's stable categorization for
                            // downstream consumers (metrics, observability, icon selection)
                            // independent of the display title we put in `name`.
                            let tool_meta = Some(serde_json::json!({
                                TOOL_META_EXTERNAL_DISPATCH_KEY: true,
                                "goose.acp.kind": kind,
                            }));
                            let message = Message::assistant().with_tool_request_with_metadata(
                                id,
                                Ok(params),
                                None,
                                tool_meta,
                            );
                            yield (Some(message), None);
                        }
                    }
                    AcpUpdate::ToolCallComplete {
                        id,
                        raw_output,
                        content,
                        is_error,
                    } => {
                        text_run = None;
                        thought_run = None;
                        if rejected_tool_calls.remove(&id) {
                            // In chat mode no tool_request was emitted (suppressed at
                            // ToolCallStart), so surface a plain text message. In other
                            // modes a tool_request WAS emitted, so pair it with an error
                            // tool_response so downstream consumers see the rejection.
                            if reject_all_tools {
                                let message = Message::assistant()
                                    .with_text("Tool call was denied.")
                                    .with_generated_id();
                                yield (Some(message), None);
                            } else {
                                let denial = vec![RmcpContent::text("Tool call was denied.")];
                                let result = CallToolResult::error(denial);
                                let message =
                                    Message::user().with_tool_response(id, Ok(result));
                                yield (Some(message), None);
                            }
                        } else {
                            let result_content =
                                acp_tool_call_content_to_rmcp(content, raw_output);
                            let result = if is_error {
                                CallToolResult::error(result_content)
                            } else {
                                CallToolResult::success(result_content)
                            };
                            let message = Message::user().with_tool_response(id, Ok(result));
                            yield (Some(message), None);
                        }
                    }
                    AcpUpdate::PermissionRequest { request, response_tx } => {
                        text_run = None;
                        thought_run = None;
                        if let Some(decision) = permission_decision_from_mode(goose_mode) {
                            if decision.should_record_rejection() {
                                rejected_tool_calls.insert(request.tool_call.tool_call_id.0.to_string());
                            }
                            let _ = response_tx.send(map_permission_response(&request, decision));
                            continue;
                        }

                        let request_id = request.tool_call.tool_call_id.0.to_string();
                        let (tx, rx) = oneshot::channel();

                        pending_confirmations
                            .lock()
                            .await
                            .insert(request_id.clone(), tx);

                        if let Some(action_required) = build_action_required_message(&request) {
                            yield (Some(action_required), None);
                        }

                        let confirmation = rx.await.unwrap_or(PermissionConfirmation {
                            principal_type: PrincipalType::Tool,
                            permission: Permission::Cancel,
                        });

                        pending_confirmations.lock().await.remove(&request_id);

                        let decision = PermissionDecision::from(confirmation.permission);
                        if decision.should_record_rejection() {
                            rejected_tool_calls.insert(request.tool_call.tool_call_id.0.to_string());
                        }
                        let _ = response_tx.send(map_permission_response(&request, decision));
                    }
                    AcpUpdate::Complete(reason, usage) => {
                        // Prefer retrying context over silently losing it. A harness may have
                        // ingested the memo before cancelling or refusing, so a retry can duplicate
                        // it, but treating an unprocessed handoff as delivered is unrecoverable.
                        if matches!(reason, StopReason::Cancelled | StopReason::Refusal) {
                            handoff_claim_guard.rollback();
                        } else {
                            handoff_claim_guard.commit();
                        }
                        if let Some(usage) = usage {
                            let provider_usage = ProviderUsage::new(
                                model_name.clone(),
                                Usage::new(
                                    Some(usage.input_tokens as i32),
                                    Some(usage.output_tokens as i32),
                                    Some(usage.total_tokens as i32),
                                ),
                            );
                            yield (None, Some(provider_usage));
                        }
                        break;
                    }
                    AcpUpdate::Error(e) => {
                        let retry_could_help = retry_without_memo_could_help(&e);
                        let error = provider_error_from_acp(e);
                        if updates_seen == 1 && retry_could_help {
                            if let Some((tx, session_id, blocks)) = bare_retry.take() {
                                // Consume the handoff before retrying. The agent has already
                                // seen and rejected this memo, so rebuilding it on a later
                                // turn would reproduce the rejection forever.
                                handoff_claim_guard.commit();
                                let (response_tx, response_rx) = mpsc::channel(64);
                                let request = ClientRequest::Prompt {
                                    session_id,
                                    content: blocks,
                                    response_tx,
                                };
                                if tx.send(request).await.is_ok() {
                                    tracing::error!(
                                        %error,
                                        "ACP prompt with handoff context rejected, retrying without it"
                                    );
                                    rx = response_rx;
                                    updates_seen = 0;
                                    continue;
                                }
                            }
                        }
                        // Reset before yielding so an immediate retry can include the handoff even
                        // while the failed stream value is still alive.
                        handoff_claim_guard.rollback();
                        Err(error)?;
                    }
                }
            }
        }))
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        let session = self.session.lock().unwrap().clone();
        let (_, available) = resolve_model_info(&self.name, &session.response)?;
        Ok(available)
    }
}

impl Drop for AcpProvider {
    fn drop(&mut self) {
        self.tx.take();
        let _cancel_tx = self.cancel_tx.take();
        if let Some(h) = self.loop_thread.take() {
            if let Err(e) = h.join() {
                tracing::debug!("AcpClientLoop thread panicked: {e:?}");
            }
        }
    }
}

struct AcpClientLoop {
    config: AcpProviderConfig,
    goose_mode: Arc<Mutex<GooseMode>>,
    prompt_response_tx: Arc<Mutex<Option<mpsc::Sender<AcpUpdate>>>>,
    pending_tool_updates: Arc<Mutex<HashMap<String, AccumulatedToolCall>>>,
    context_size: Arc<AtomicU64>,
    effort: AcpEffortState,
}

impl AcpClientLoop {
    fn new(
        config: AcpProviderConfig,
        goose_mode: Arc<Mutex<GooseMode>>,
        pending_tool_updates: Arc<Mutex<HashMap<String, AccumulatedToolCall>>>,
        context_size: Arc<AtomicU64>,
        effort: AcpEffortState,
    ) -> Self {
        Self {
            config,
            goose_mode,
            prompt_response_tx: Arc::new(Mutex::new(None)),
            pending_tool_updates,
            context_size,
            effort,
        }
    }

    async fn spawn(
        self,
        mut rx: mpsc::Receiver<ClientRequest>,
        init_tx: oneshot::Sender<Result<InitializeResponse>>,
    ) {
        let child = match spawn_acp_process(&self.config).await {
            Ok(c) => c,
            Err(e) => {
                let _ = init_tx.send(Err(anyhow::anyhow!("{e}")));
                tracing::error!("failed to spawn ACP process: {e}");
                return;
            }
        };

        match self.run_with_child(child, &mut rx, init_tx).await {
            Ok(()) => tracing::debug!("ACP protocol loop exited cleanly"),
            Err(e) => tracing::error!(error = %e, "ACP protocol loop error"),
        }
    }

    async fn run_with_child(
        self,
        mut child: Child,
        rx: &mut mpsc::Receiver<ClientRequest>,
        init_tx: oneshot::Sender<Result<InitializeResponse>>,
    ) -> Result<()> {
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_child_stderr(stderr));
        }
        let transport =
            agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat());
        let result = self.run(transport, rx, init_tx).await;
        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    }

    async fn run(
        self,
        transport: impl agent_client_protocol::ConnectTo<Client> + 'static,
        rx: &mut mpsc::Receiver<ClientRequest>,
        init_tx: oneshot::Sender<Result<InitializeResponse>>,
    ) -> Result<()> {
        let AcpClientLoop {
            config,
            goose_mode,
            prompt_response_tx,
            pending_tool_updates,
            context_size,
            effort,
        } = self;
        let notification_callback = config.notification_callback.clone();
        let reverse_modes = reverse_mode_mapping(&config.mode_mapping);
        let session_state = AcpSessionState::new(effort);

        Client
            .builder()
            .on_receive_notification(
                {
                    let prompt_response_tx = prompt_response_tx.clone();
                    let reverse_modes = reverse_modes.clone();
                    let goose_mode = goose_mode.clone();
                    let pending_tool_updates = pending_tool_updates.clone();
                    let context_size = context_size.clone();
                    let session_state = session_state.clone();
                    async move |notification: SessionNotification, _cx| {
                        let is_active_session =
                            session_state.active_id.lock().is_ok_and(|active| {
                                active
                                    .as_ref()
                                    .is_none_or(|id| id == &notification.session_id)
                            });
                        if !is_active_session {
                            return Ok(());
                        }
                        if let Some(ref cb) = notification_callback {
                            cb(notification.clone());
                        }
                        match &notification.update {
                            SessionUpdate::CurrentModeUpdate(update) => {
                                if let Some(mode) = resolve_mode(
                                    &reverse_modes,
                                    update.current_mode_id.0.as_ref(),
                                    &goose_mode,
                                ) {
                                    if let Ok(mut guard) = goose_mode.lock() {
                                        *guard = mode;
                                    }
                                }
                            }
                            SessionUpdate::ConfigOptionUpdate(update) => {
                                publish_effort_state(&session_state.effort, &update.config_options);
                                for opt in &update.config_options {
                                    if opt.category == Some(SessionConfigOptionCategory::Mode) {
                                        if let SessionConfigKind::Select(sel) = &opt.kind {
                                            if let Some(mode) = resolve_mode(
                                                &reverse_modes,
                                                sel.current_value.0.as_ref(),
                                                &goose_mode,
                                            ) {
                                                if let Ok(mut guard) = goose_mode.lock() {
                                                    *guard = mode;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            SessionUpdate::UsageUpdate(usage) => {
                                context_size.store(usage.size, Ordering::Relaxed);
                            }
                            _ => {}
                        }
                        if let Some(tx) = prompt_response_tx
                            .lock()
                            .ok()
                            .as_ref()
                            .and_then(|g| g.as_ref())
                        {
                            match notification.update {
                                SessionUpdate::AgentMessageChunk(ContentChunk {
                                    content: ContentBlock::Text(text),
                                    ..
                                }) => {
                                    let _ = tx.try_send(AcpUpdate::Text(text));
                                }
                                SessionUpdate::AgentThoughtChunk(ContentChunk {
                                    content: ContentBlock::Text(TextContent { text, .. }),
                                    ..
                                }) => {
                                    let _ = tx.try_send(AcpUpdate::Thought(text));
                                }
                                SessionUpdate::ToolCall(tool_call) => {
                                    let id = tool_call.tool_call_id.0.to_string();
                                    let initial_status = tool_call.status;
                                    let synchronous_terminal = matches!(
                                        initial_status,
                                        ToolCallStatus::Completed | ToolCallStatus::Failed
                                    );
                                    // Seed the buffer; drain immediately if the call is
                                    // already terminal (synchronous tool, no follow-up).
                                    let synchronous_accumulated =
                                        if let Ok(mut buffer) = pending_tool_updates.lock() {
                                            let entry = buffer.entry(id.clone()).or_default();
                                            if let Some(raw_output) = tool_call.raw_output.clone() {
                                                entry.raw_output = Some(raw_output);
                                            }
                                            entry.content.extend(tool_call.content.clone());
                                            if synchronous_terminal {
                                                buffer.remove(&id)
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        };
                                    // ACP carries no canonical tool name to clients — only
                                    // `title` (display) and `kind` (category). We pass `title`
                                    // for renderer affordance, surface `kind` separately via
                                    // tool_meta for stable categorization, and the
                                    // goose.external_dispatch marker keeps `name` off the
                                    // agent loop's routing/auth paths.
                                    let _ = tx.try_send(AcpUpdate::ToolCallStart {
                                        id: id.clone(),
                                        name: tool_call.title.clone(),
                                        kind: tool_call.kind,
                                        raw_input: tool_call.raw_input.clone(),
                                    });
                                    if let Some(accumulated) = synchronous_accumulated {
                                        let content = if accumulated.content.is_empty() {
                                            None
                                        } else {
                                            Some(accumulated.content)
                                        };
                                        let _ = tx.try_send(AcpUpdate::ToolCallComplete {
                                            id,
                                            raw_output: accumulated.raw_output,
                                            content,
                                            is_error: matches!(
                                                initial_status,
                                                ToolCallStatus::Failed
                                            ),
                                        });
                                    }
                                }
                                SessionUpdate::ToolCallUpdate(update) => {
                                    let id = update.tool_call_id.0.to_string();
                                    // Merge patch-like fields; only emit on terminal status.
                                    let terminal_status = update.fields.status.filter(|s| {
                                        matches!(
                                            s,
                                            ToolCallStatus::Completed | ToolCallStatus::Failed
                                        )
                                    });
                                    let accumulated = if let Ok(mut buffer) =
                                        pending_tool_updates.lock()
                                    {
                                        let entry = buffer.entry(id.clone()).or_default();
                                        if let Some(raw_output) = update.fields.raw_output.clone() {
                                            entry.raw_output = Some(raw_output);
                                        }
                                        if let Some(content) = update.fields.content.clone() {
                                            entry.content.extend(content);
                                        }
                                        if terminal_status.is_some() {
                                            buffer.remove(&id)
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    if let (Some(accumulated), Some(status)) =
                                        (accumulated, terminal_status)
                                    {
                                        let content = if accumulated.content.is_empty() {
                                            None
                                        } else {
                                            Some(accumulated.content)
                                        };
                                        let _ = tx.try_send(AcpUpdate::ToolCallComplete {
                                            id,
                                            raw_output: accumulated.raw_output,
                                            content,
                                            is_error: matches!(status, ToolCallStatus::Failed),
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let prompt_response_tx = prompt_response_tx.clone();
                    async move |request: RequestPermissionRequest, responder, _connection_cx| {
                        let (response_tx, response_rx) = oneshot::channel();

                        let handler = prompt_response_tx
                            .lock()
                            .ok()
                            .as_ref()
                            .and_then(|g| g.as_ref().cloned());
                        let tx =
                            handler.ok_or_else(agent_client_protocol::Error::internal_error)?;

                        if tx.is_closed() {
                            return Err(agent_client_protocol::Error::internal_error());
                        }

                        tx.try_send(AcpUpdate::PermissionRequest {
                            request: Box::new(request),
                            response_tx,
                        })
                        .map_err(|_| agent_client_protocol::Error::internal_error())?;

                        let response = response_rx.await.unwrap_or_else(|_| {
                            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                        });
                        responder.respond(response)
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                handle_requests(
                    config,
                    goose_mode,
                    cx,
                    rx,
                    prompt_response_tx,
                    session_state,
                    init_tx,
                )
                .await
            })
            .await?;

        Ok(())
    }
}

/// Forwards an ACP child's stderr to tracing line by line.
///
/// Lines longer than `MAX_LINE_LEN` are flushed in chunks so a child that
/// emits unbounded output without newlines (e.g. carriage-return progress
/// bars or binary data) cannot cause unbounded memory growth.
async fn forward_child_stderr(mut stderr: tokio::process::ChildStderr) {
    const MAX_LINE_LEN: usize = 8192;
    const READ_CHUNK: usize = 1024;

    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                for &b in &chunk[..n] {
                    if b == b'\n' {
                        emit_stderr_line(&mut line);
                    } else {
                        line.push(b);
                        if line.len() >= MAX_LINE_LEN {
                            emit_stderr_line(&mut line);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::debug!(target: "goose::acp::child::stderr", error = %e, "stderr read error");
                break;
            }
        }
    }
    emit_stderr_line(&mut line);
}

fn emit_stderr_line(line: &mut Vec<u8>) {
    if line.is_empty() {
        return;
    }
    let trimmed = line.strip_suffix(b"\r").unwrap_or(line);
    tracing::info!(target: "goose::acp::child::stderr", "{}", String::from_utf8_lossy(trimmed));
    line.clear();
}

async fn spawn_acp_process(config: &AcpProviderConfig) -> Result<Child> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(command_dir) = config.command.parent() {
        // npm adapters commonly use `/usr/bin/env node`, while desktop PATH may omit their bin dir.
        let path = std::env::join_paths(
            std::iter::once(command_dir.to_path_buf()).chain(
                std::env::var_os("PATH")
                    .as_ref()
                    .map(std::env::split_paths)
                    .into_iter()
                    .flatten(),
            ),
        )?;
        cmd.env("PATH", path);
    }

    for key in &config.env_remove {
        cmd.env_remove(key);
    }

    for (key, value) in &config.env {
        cmd.env(key, value);
    }

    configure_subprocess(&mut cmd);
    cmd.spawn().context("failed to spawn ACP process")
}

fn log_undelivered<E: std::fmt::Debug>(result: Result<(), E>, method: &str) {
    if let Err(e) = result {
        tracing::debug!(method, error = ?e, "response not delivered");
    }
}

fn acp_method_error(method: &str, error: agent_client_protocol::Error) -> anyhow::Error {
    let message = format!("ACP {method} failed: {error}");
    anyhow::Error::new(error).context(message)
}

async fn handle_requests(
    config: AcpProviderConfig,
    goose_mode: Arc<Mutex<GooseMode>>,
    cx: ConnectionTo<Agent>,
    rx: &mut mpsc::Receiver<ClientRequest>,
    prompt_response_tx: Arc<Mutex<Option<mpsc::Sender<AcpUpdate>>>>,
    session_state: AcpSessionState,
    init_tx: oneshot::Sender<Result<InitializeResponse>>,
) -> Result<(), agent_client_protocol::Error> {
    let mut init_tx = Some(init_tx);

    let client_capabilities = ClientCapabilities::new();
    let init_response: InitializeResponse = cx
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client_capabilities),
        )
        .block_task()
        .await
        .map_err(|err| {
            let message = format!("ACP {} failed: {err}", AGENT_METHOD_NAMES.initialize);
            if let Some(tx) = init_tx.take() {
                let _ = tx.send(Err(anyhow::anyhow!(message.clone())));
            }
            agent_client_protocol::Error::internal_error().data(message)
        })?;

    let supports_close = init_response
        .agent_capabilities
        .session_capabilities
        .close
        .is_some();
    let supports_load = init_response.agent_capabilities.load_session;
    let mcp_capabilities = init_response.agent_capabilities.mcp_capabilities.clone();
    if let Some(tx) = init_tx.take() {
        log_undelivered(tx.send(Ok(init_response)), AGENT_METHOD_NAMES.initialize);
    }

    let mut session_ids: Vec<SessionId> = Vec::new();

    while let Some(request) = rx.recv().await {
        match request {
            ClientRequest::NewSession { response_tx } => {
                let mcp_servers = filter_supported_servers(&config.mcp_servers, &mcp_capabilities);
                let session = cx
                    .send_request(
                        NewSessionRequest::new(config.work_dir.clone()).mcp_servers(mcp_servers),
                    )
                    .block_task()
                    .await;
                let result = match session {
                    Ok(session) => {
                        *session_state.active_id.lock().unwrap() = Some(session.session_id.clone());
                        session_ids.push(session.session_id.clone());
                        if let Some(config_options) = session.config_options.as_deref() {
                            publish_effort_state(&session_state.effort, config_options);
                        }
                        apply_session_config_options(
                            &config,
                            &cx,
                            session.session_id.clone(),
                            &session_state.effort,
                        )
                        .await?;
                        apply_session_mode(&config, &goose_mode, &cx, session).await
                    }
                    Err(error) => Err(acp_method_error(AGENT_METHOD_NAMES.session_new, error)),
                };
                log_undelivered(response_tx.send(result), AGENT_METHOD_NAMES.session_new);
            }
            ClientRequest::LoadSession {
                session_id,
                response_tx,
            } => {
                let previous_session_id = session_state
                    .active_id
                    .lock()
                    .unwrap()
                    .replace(session_id.clone());
                let previous_effort_state = replace_effort_state(&session_state.effort, None);
                let result = if supports_load {
                    let mcp_servers =
                        filter_supported_servers(&config.mcp_servers, &mcp_capabilities);
                    cx.send_request(
                        LoadSessionRequest::new(session_id.clone(), config.work_dir.clone())
                            .mcp_servers(mcp_servers),
                    )
                    .block_task()
                    .await
                    .map(|response| {
                        NewSessionResponse::new(session_id.clone())
                            .modes(response.modes)
                            .config_options(response.config_options)
                            .meta(response.meta)
                    })
                    .map_err(anyhow::Error::from)
                } else {
                    Err(anyhow::anyhow!("ACP agent does not support session/load"))
                };
                let result = match result {
                    Ok(session) => {
                        session_ids.push(session.session_id.clone());
                        if let Some(config_options) = session.config_options.as_deref() {
                            publish_effort_state(&session_state.effort, config_options);
                        }
                        apply_session_config_options(
                            &config,
                            &cx,
                            session.session_id.clone(),
                            &session_state.effort,
                        )
                        .await?;
                        apply_session_mode(&config, &goose_mode, &cx, session).await
                    }
                    Err(error) => {
                        *session_state.active_id.lock().unwrap() = previous_session_id;
                        replace_effort_state(&session_state.effort, previous_effort_state);
                        Err(error)
                    }
                };
                log_undelivered(response_tx.send(result), AGENT_METHOD_NAMES.session_load);
            }
            ClientRequest::CloseSession { session_id } => {
                if supports_close {
                    if let Err(error) = cx
                        .send_request(CloseSessionRequest::new(session_id.clone()))
                        .block_task()
                        .await
                    {
                        tracing::debug!(method = AGENT_METHOD_NAMES.session_close, session_id = %session_id, %error, "failed to close replaced ACP session");
                    }
                }
                session_ids.retain(|id| id != &session_id);
            }
            ClientRequest::SetMode {
                session_id,
                mode_id,
                response_tx,
            } => {
                let result: Result<()> = cx
                    .send_request(SetSessionModeRequest::new(session_id, mode_id))
                    .block_task()
                    .await
                    .map(|_| ())
                    .map_err(anyhow::Error::from);
                log_undelivered(
                    response_tx.send(result),
                    AGENT_METHOD_NAMES.session_set_mode,
                );
            }
            ClientRequest::SetConfigOption {
                session_id,
                config_id,
                value,
                response_tx,
            } => {
                let value_id = agent_client_protocol::schema::v1::SessionConfigValueId::new(value);
                let req = SetSessionConfigOptionRequest::new(session_id, config_id, value_id);
                // The agent rebuilds per-model effort levels in this response,
                // so it is the freshest source after a goose-initiated switch.
                let result: Result<()> = cx
                    .send_request(req)
                    .block_task()
                    .await
                    .map(|response| {
                        publish_effort_state(&session_state.effort, &response.config_options)
                    })
                    .map_err(anyhow::Error::from);
                log_undelivered(
                    response_tx.send(result),
                    AGENT_METHOD_NAMES.session_set_config_option,
                );
            }
            ClientRequest::Prompt {
                session_id,
                content,
                response_tx,
            } => {
                *prompt_response_tx.lock().unwrap() = Some(response_tx.clone());

                let response: Result<PromptResponse, _> = cx
                    .send_request(PromptRequest::new(session_id, content))
                    .block_task()
                    .await;

                match response {
                    Ok(r) => {
                        log_undelivered(
                            response_tx.try_send(AcpUpdate::Complete(r.stop_reason, r.usage)),
                            AGENT_METHOD_NAMES.session_prompt,
                        );
                    }
                    Err(e) => {
                        log_undelivered(
                            response_tx.try_send(AcpUpdate::Error(e)),
                            AGENT_METHOD_NAMES.session_prompt,
                        );
                    }
                }

                *prompt_response_tx.lock().unwrap() = None;
            }
        }
    }

    if supports_close && !supports_load {
        for session_id in session_ids {
            if let Err(e) = cx
                .send_request(CloseSessionRequest::new(session_id.clone()))
                .block_task()
                .await
            {
                tracing::debug!(method = AGENT_METHOD_NAMES.session_close, session_id = %session_id, error = %e, "failed on shutdown");
            }
        }
    }

    Ok(())
}

async fn apply_session_config_options(
    config: &AcpProviderConfig,
    cx: &ConnectionTo<Agent>,
    session_id: SessionId,
    effort: &AcpEffortState,
) -> Result<()> {
    for (config_id, value) in &config.session_config_options {
        let value_id = agent_client_protocol::schema::v1::SessionConfigValueId::new(value.clone());
        let response = cx
            .send_request(SetSessionConfigOptionRequest::new(
                session_id.clone(),
                config_id.clone(),
                value_id,
            ))
            .block_task()
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "ACP agent rejected {} for '{}': {err}",
                    AGENT_METHOD_NAMES.session_set_config_option,
                    config_id
                )
            })?;
        // Pinning the model here makes the agent rebuild its per-model effort
        // levels, so this response supersedes the session/new snapshot.
        publish_effort_state(effort, &response.config_options);
    }
    Ok(())
}

async fn apply_session_mode(
    config: &AcpProviderConfig,
    goose_mode: &Arc<Mutex<GooseMode>>,
    cx: &ConnectionTo<Agent>,
    session: NewSessionResponse,
) -> Result<NewSessionResponse> {
    let current_mode = goose_mode.lock().ok().map(|mode| *mode);
    let candidates = initial_mode_candidates(config, current_mode);

    if let Some(modes) = session.modes.as_ref() {
        if !candidates.is_empty() {
            let Some(mode_id) = select_mode_id(&candidates, Some(modes)) else {
                let available: Vec<String> = modes
                    .available_modes
                    .iter()
                    .map(|mode| mode.id.0.to_string())
                    .collect();
                return Err(anyhow::anyhow!(
                    "Requested mode(s) [{}] not offered by agent. Available modes: {}",
                    candidates.join(", "),
                    available.join(", ")
                ));
            };
            if modes.current_mode_id.0.as_ref() != mode_id.as_str() {
                let _: SetSessionModeResponse = cx
                    .send_request(SetSessionModeRequest::new(
                        session.session_id.clone(),
                        mode_id,
                    ))
                    .block_task()
                    .await
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "ACP agent rejected {}: {err}",
                            AGENT_METHOD_NAMES.session_set_mode
                        )
                    })?;
            }
        }
    }

    Ok(session)
}

fn initial_mode_candidates(
    config: &AcpProviderConfig,
    current_mode: Option<GooseMode>,
) -> Vec<String> {
    current_mode
        .and_then(|mode| config.mode_mapping.get(&mode).cloned())
        .or_else(|| config.session_mode_id.clone().map(|id| vec![id]))
        .unwrap_or_default()
}

fn select_mode_id(candidates: &[String], modes: Option<&SessionModeState>) -> Option<String> {
    match modes {
        Some(state) => candidates
            .iter()
            .find(|candidate| {
                state
                    .available_modes
                    .iter()
                    .any(|mode| mode.id.0.as_ref() == candidate.as_str())
            })
            .cloned(),
        None => candidates.first().cloned(),
    }
}

pub fn extension_configs_to_mcp_servers(configs: &[ExtensionConfig]) -> Vec<McpServer> {
    let mut servers = Vec::new();

    for config in configs {
        match config {
            ExtensionConfig::StreamableHttp {
                name,
                socket: Some(_),
                ..
            } => {
                tracing::debug!(
                    name,
                    "skipping socket-backed HTTP extension, unsupported by ACP"
                );
            }
            ExtensionConfig::StreamableHttp {
                name,
                uri,
                headers,
                socket: None,
                ..
            } => {
                let http_headers = headers
                    .iter()
                    .map(|(key, value)| HttpHeader::new(key, value))
                    .collect();
                servers.push(McpServer::Http(
                    McpServerHttp::new(name, uri).headers(http_headers),
                ));
            }
            ExtensionConfig::Stdio {
                name,
                cmd,
                args,
                envs,
                ..
            } => {
                let env_vars = envs
                    .get_env()
                    .into_iter()
                    .map(|(key, value)| EnvVariable::new(key, value))
                    .collect();

                servers.push(McpServer::Stdio(
                    McpServerStdio::new(name, cmd)
                        .args(args.clone())
                        .env(env_vars),
                ));
            }
            _ => {}
        }
    }

    servers
}

fn filter_supported_servers(
    servers: &[McpServer],
    capabilities: &McpCapabilities,
) -> Vec<McpServer> {
    servers
        .iter()
        .filter(|server| match server {
            McpServer::Http(http) => {
                if !capabilities.http {
                    tracing::debug!(
                        name = http.name,
                        "skipping HTTP server, agent lacks capability"
                    );
                    false
                } else {
                    true
                }
            }
            McpServer::Sse(sse) => {
                tracing::debug!(name = sse.name, "skipping SSE server, unsupported");
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

fn messages_to_prompt(messages: &[Message], handoff_memo: Option<String>) -> Vec<ContentBlock> {
    let Some(last_user_index) = last_user_message_index(messages) else {
        return Vec::new();
    };

    let message = messages[last_user_index].agent_visible_content();
    let mut current_prompt_blocks = Vec::new();
    for content in &message.content {
        match content {
            MessageContent::Text(text) => {
                current_prompt_blocks.push(ContentBlock::Text(TextContent::new(text.text.clone())));
            }
            MessageContent::Image(image) => {
                current_prompt_blocks.push(ContentBlock::Image(ImageContent::new(
                    &image.data,
                    &image.mime_type,
                )));
            }
            _ => {}
        }
    }

    let Some(memo) = handoff_memo else {
        return current_prompt_blocks;
    };
    if current_prompt_blocks.is_empty() {
        return current_prompt_blocks;
    }

    let mut content_blocks = vec![ContentBlock::Text(TextContent::new(memo))];
    content_blocks.extend(current_prompt_blocks);
    content_blocks
}

fn last_user_message_index(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == Role::User && m.is_agent_visible() && !m.is_turn_context())
}

fn has_handoff_context(messages: &[Message]) -> bool {
    last_user_message_index(messages).is_some_and(|last_user_index| {
        messages[..last_user_index]
            .iter()
            .any(|m| m.is_agent_visible() && !m.is_turn_context())
    })
}

fn acp_audience_to_rmcp(annotations: Option<&AcpAnnotations>) -> Option<Vec<Role>> {
    let audience = annotations?.audience.as_ref()?;
    let audience = audience
        .iter()
        .filter_map(|role| match role {
            AcpRole::Assistant => Some(Role::Assistant),
            AcpRole::User => Some(Role::User),
            _ => None,
        })
        .collect::<Vec<_>>();

    if audience.is_empty() {
        None
    } else {
        Some(audience)
    }
}

fn acp_annotations_to_rmcp(annotations: Option<&AcpAnnotations>) -> rmcp::model::Annotations {
    let mut rmcp_annotations = rmcp::model::Annotations::default().with_priority(0.0);
    if let Some(audience) = acp_audience_to_rmcp(annotations) {
        rmcp_annotations = rmcp_annotations.with_audience(audience);
    }
    rmcp_annotations
}

fn acp_text_content_to_rmcp(text: TextContent) -> RmcpContent {
    RmcpContent::Text(
        rmcp::model::TextContent::new(sanitize_unicode_tags(&text.text))
            .with_annotations(acp_annotations_to_rmcp(text.annotations.as_ref())),
    )
}

fn acp_image_content_to_rmcp(image: ImageContent) -> RmcpContent {
    RmcpContent::Image(
        rmcp::model::ImageContent::new(image.data, image.mime_type)
            .with_annotations(acp_annotations_to_rmcp(image.annotations.as_ref())),
    )
}

fn visible_rmcp_text(text: impl Into<String>) -> RmcpContent {
    visible_rmcp_text_with_annotations(text, None)
}

fn visible_rmcp_text_with_annotations(
    text: impl Into<String>,
    annotations: Option<&AcpAnnotations>,
) -> RmcpContent {
    RmcpContent::Text(
        rmcp::model::TextContent::new(text).with_annotations(acp_annotations_to_rmcp(annotations)),
    )
}

fn acp_content_annotations(content: &ContentBlock) -> Option<&AcpAnnotations> {
    match content {
        ContentBlock::Text(content) => content.annotations.as_ref(),
        ContentBlock::Image(content) => content.annotations.as_ref(),
        ContentBlock::Audio(content) => content.annotations.as_ref(),
        ContentBlock::ResourceLink(content) => content.annotations.as_ref(),
        ContentBlock::Resource(content) => content.annotations.as_ref(),
        _ => None,
    }
}

fn acp_text_update_message(text: TextContent, id: String, created: i64) -> Message {
    Message::new(Role::Assistant, created, vec![])
        .with_content(acp_text_content_to_rmcp(text).into())
        .with_id(id)
}

/// Convert ACP `ToolCallContent` blocks into the rmcp `Content` shape goose's
/// `Message::with_tool_response` consumes. Handles `Content` (text/image/other),
/// `Diff`, and `Terminal` variants; falls back to a JSON serialization of
/// `raw_output` when no blocks are present so the renderer always has something.
fn acp_tool_call_content_to_rmcp(
    content: Option<Vec<ToolCallContent>>,
    raw_output: Option<serde_json::Value>,
) -> Vec<RmcpContent> {
    let mut out = Vec::new();
    if let Some(blocks) = content {
        for block in blocks {
            match block {
                ToolCallContent::Content(val) => match val.content {
                    ContentBlock::Text(text) => {
                        out.push(acp_text_content_to_rmcp(text));
                    }
                    ContentBlock::Image(image) => {
                        out.push(acp_image_content_to_rmcp(image));
                    }
                    other => {
                        if let Ok(json) = serde_json::to_string(&other) {
                            out.push(visible_rmcp_text_with_annotations(
                                json,
                                acp_content_annotations(&other),
                            ));
                        }
                    }
                },
                ToolCallContent::Diff(diff) => {
                    let path = diff.path.display();
                    let body = match diff.old_text.as_deref() {
                        Some(old) => {
                            format!("--- {path}\n{old}\n+++ {path}\n{}", diff.new_text)
                        }
                        None => format!("+++ {path}\n{}", diff.new_text),
                    };
                    out.push(visible_rmcp_text(body));
                }
                ToolCallContent::Terminal(_) => {}
                _ => {}
            }
        }
    }
    if out.is_empty() {
        if let Some(raw) = raw_output {
            let text = match raw {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            out.push(visible_rmcp_text(text));
        }
    }
    out
}

fn build_action_required_message(request: &RequestPermissionRequest) -> Option<Message> {
    let tool_title = request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "Tool".to_string());

    let arguments = request
        .tool_call
        .fields
        .raw_input
        .as_ref()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let prompt = request
        .tool_call
        .fields
        .content
        .as_ref()
        .and_then(|content| {
            content.iter().find_map(|c| match c {
                ToolCallContent::Content(val) => match &val.content {
                    ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                },
                _ => None,
            })
        });

    Some(
        Message::assistant()
            .with_action_required(
                request.tool_call.tool_call_id.0.to_string(),
                tool_title,
                arguments,
                prompt,
            )
            .user_only(),
    )
}

fn extract_model_info_from_config_options(
    config_options: &[SessionConfigOption],
) -> Option<(String, Vec<String>)> {
    let select = config_options.iter().find_map(|opt| {
        if opt.category.as_ref() != Some(&SessionConfigOptionCategory::Model) {
            return None;
        }
        match &opt.kind {
            SessionConfigKind::Select(select) => Some(select),
            _ => None,
        }
    })?;

    let current = select.current_value.0.to_string();
    let available = select_option_values(&select.options)
        .into_iter()
        .map(|option| option.value)
        .collect();
    Some((current, available))
}

fn resolve_model_info(
    provider_name: &str,
    response: &NewSessionResponse,
) -> Result<(String, Vec<String>), ProviderError> {
    if let Some(opts) = &response.config_options {
        if let Some((current, available)) = extract_model_info_from_config_options(opts) {
            return Ok((current, available));
        }
    }

    Err(ProviderError::RequestFailed(format!(
        "{provider_name}: agent returned no model config_options"
    )))
}

fn select_option_values(options: &SessionConfigSelectOptions) -> Vec<ThinkingEffortOption> {
    let effort_option = |option: &SessionConfigSelectOption| ThinkingEffortOption {
        value: option.value.0.to_string(),
        label: option.name.clone(),
    };
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().map(effort_option).collect()
        }
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(effort_option))
            .collect(),
        _ => Vec::new(),
    }
}

/// Find the agent's thinking-effort selector. Detection is by category so any
/// ACP agent advertising `thought_level` is supported without per-provider
/// config, with the well-known option id as a fallback.
fn extract_effort_capability(
    config_options: &[SessionConfigOption],
) -> Option<ThinkingEffortCapability> {
    let option = config_options
        .iter()
        .find(|opt| opt.category.as_ref() == Some(&SessionConfigOptionCategory::ThoughtLevel))
        .or_else(|| {
            config_options
                .iter()
                .find(|opt| opt.id.0.as_ref() == EFFORT_CONFIG_OPTION_ID)
        })?;
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };

    let values = select_option_values(&select.options);
    if values.is_empty() {
        return None;
    }
    Some(ThinkingEffortCapability {
        option_id: option.id.0.to_string(),
        values,
        current: Some(select.current_value.0.to_string()),
    })
}

/// Config-options payloads carry the agent's full set, so a payload without an
/// effort selector means the current model has none and the mirrored capability
/// must be dropped.
#[cfg(test)]
fn refresh_effort_state(
    effort_state: &Arc<Mutex<Option<ThinkingEffortCapability>>>,
    config_options: &[SessionConfigOption],
) {
    if let Ok(mut state) = effort_state.lock() {
        *state = extract_effort_capability(config_options);
    }
}

fn publish_effort_support(
    effort_updates: &watch::Sender<ThinkingEffortSupport>,
    support: ThinkingEffortSupport,
) {
    effort_updates.send_if_modified(|current| {
        if *current == support {
            false
        } else {
            *current = support;
            true
        }
    });
}

fn publish_effort_state(effort: &AcpEffortState, config_options: &[SessionConfigOption]) {
    let capability = extract_effort_capability(config_options);
    replace_effort_state(effort, capability);
}

fn replace_effort_state(
    effort: &AcpEffortState,
    capability: Option<ThinkingEffortCapability>,
) -> Option<ThinkingEffortCapability> {
    let mut state = effort.capability.lock().unwrap();
    let previous = std::mem::replace(&mut *state, capability.clone());
    publish_effort_support(
        &effort.updates,
        capability.map_or(
            ThinkingEffortSupport::Unsupported,
            ThinkingEffortSupport::Options,
        ),
    );
    previous
}

/// Map a goose effort value onto the agent's advertised vocabulary. Values goose
/// and the agent share pass through; goose's own enum values map onto their
/// closest agent equivalent. Anything else yields `None` so we never send a
/// value the agent would reject.
pub(super) fn map_effort_value(
    capability: &ThinkingEffortCapability,
    value: &str,
) -> Option<String> {
    let offered = |candidate: &str| {
        capability
            .values
            .iter()
            .find(|option| option.value.eq_ignore_ascii_case(candidate))
            .map(|option| option.value.clone())
    };

    offered(value).or_else(|| {
        let synonyms: &[&str] = match value.to_lowercase().as_str() {
            // Harnesses that always reason have no "off"; their default is the
            // closest thing to not forcing an effort level.
            "off" => &["default"],
            "max" => &["xhigh"],
            "xhigh" => &["max"],
            _ => &[],
        };
        synonyms.iter().find_map(|candidate| offered(candidate))
    })
}

/// Resolve the goose-side thinking effort to send to the agent: the session's
/// persisted pick wins, then the global default, each mapped into the agent's
/// vocabulary. `None` leaves the agent on its own current value. Shared with the
/// config menu so the advertised selection is the applied one — a persisted pick
/// the agent no longer offers (its selector was rebuilt by a model switch) falls
/// back to the global on both sides. The unmappable pick is deliberately left
/// persisted: it becomes honorable again if the user switches back.
pub(super) fn resolve_effort_value(
    capability: &ThinkingEffortCapability,
    model_config: &ModelConfig,
) -> Option<String> {
    model_config
        .request_param::<String>(THINKING_EFFORT_PARAM)
        .and_then(|value| map_effort_value(capability, &value))
        .or_else(|| {
            Config::global()
                .get_goose_thinking_effort()
                .and_then(|effort| map_effort_value(capability, &effort.to_string()))
        })
}

/// Separate a value the agent evaluated and refused from an operational failure.
/// Only an agent that actually processed the request answers with the JSON-RPC
/// `invalid_params` code; the client library synthesizes `internal_error` for a
/// dead subprocess or dropped connection, and goose's own send failures carry no
/// ACP error at all.
fn effort_option_error(value: &str, error: anyhow::Error) -> ProviderError {
    match error.downcast_ref::<agent_client_protocol::Error>() {
        Some(acp_error) if acp_error.code == agent_client_protocol::ErrorCode::InvalidParams => {
            ProviderError::InvalidValue(format!(
                "Agent rejected thinking effort '{value}': {acp_error}"
            ))
        }
        _ => ProviderError::RequestFailed(format!("Failed to set ACP effort option: {error}")),
    }
}

fn reverse_mode_mapping(
    mode_mapping: &HashMap<GooseMode, Vec<String>>,
) -> HashMap<String, Vec<GooseMode>> {
    let mut reverse: HashMap<String, Vec<GooseMode>> = HashMap::new();
    for (mode, ids) in mode_mapping {
        for id in ids {
            reverse.entry(id.clone()).or_default().push(*mode);
        }
    }
    reverse
}

fn resolve_mode(
    reverse_modes: &HashMap<String, Vec<GooseMode>>,
    mode_id: &str,
    current: &Arc<Mutex<GooseMode>>,
) -> Option<GooseMode> {
    let candidates = reverse_modes.get(mode_id)?;
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }
    let current = current.lock().ok()?;
    if candidates.contains(&*current) {
        Some(*current)
    } else {
        Some(candidates[0])
    }
}

fn permission_decision_from_mode(goose_mode: GooseMode) -> Option<PermissionDecision> {
    match goose_mode {
        GooseMode::Auto => Some(PermissionDecision::AllowOnce),
        GooseMode::Chat => Some(PermissionDecision::RejectOnce),
        GooseMode::Approve | GooseMode::SmartApprove => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::extension::Envs;
    use agent_client_protocol::schema::v1::{
        ConfigOptionUpdate, ErrorCode, SessionConfigSelectGroup, SessionConfigSelectOption,
        SessionMode, SessionModeId,
    };

    use test_case::test_case;

    fn prompt_text(block: &ContentBlock) -> &str {
        match block {
            ContentBlock::Text(text) => &text.text,
            _ => panic!("expected text block"),
        }
    }

    fn acp_error_code(error: &anyhow::Error) -> Option<ErrorCode> {
        error.chain().find_map(|source| {
            source
                .downcast_ref::<agent_client_protocol::Error>()
                .map(|error| error.code)
        })
    }

    /// The prompt as `stream` builds it on a first prompt, with a budget generous
    /// enough that nothing is dropped. Memo bounding is covered in `acp::handoff`.
    async fn prompt_with_handoff(messages: &[Message]) -> Vec<ContentBlock> {
        let counter = crate::token_counter::create_token_counter().await.unwrap();
        let memo = last_user_message_index(messages)
            .and_then(|index| build_handoff_context_memo(&messages[..index], 50_000, &counter));
        messages_to_prompt(messages, memo)
    }

    #[test]
    fn startup_cleanup_does_not_block_while_the_client_loop_stops() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let client_thread = std::thread::spawn(move || release_rx.recv().unwrap());
        let guard = ClientLoopGuard {
            tx: None,
            cancel_tx: None,
            thread: Some(client_thread),
        };
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let drop_thread = std::thread::spawn(move || {
            drop(guard);
            dropped_tx.send(()).unwrap();
        });

        let returned_without_joining = dropped_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_ok();
        release_tx.send(()).unwrap();
        drop_thread.join().unwrap();

        assert!(returned_without_joining);
    }

    #[test]
    fn provider_drop_closes_requests_without_forcing_cancellation() {
        let (tx, mut rx) = mpsc::channel(1);
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let (graceful_tx, graceful_rx) = std::sync::mpsc::channel();
        let client_thread = std::thread::spawn(move || {
            while rx.blocking_recv().is_some() {}
            graceful_tx
                .send(matches!(
                    cancel_rx.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ))
                .unwrap();
        });
        let (mut provider, _) = test_provider_with_tx(Some(tx));
        provider.cancel_tx = Some(cancel_tx);
        provider.loop_thread = Some(client_thread);

        drop(provider);

        assert!(graceful_rx.recv().unwrap());
    }

    #[test]
    fn session_new_error_preserves_auth_required_code() {
        let error = acp_method_error(
            AGENT_METHOD_NAMES.session_new,
            agent_client_protocol::Error::auth_required(),
        );

        assert_eq!(
            error.to_string(),
            "ACP session/new failed: Authentication required"
        );
        assert_eq!(acp_error_code(&error), Some(ErrorCode::AuthRequired));
    }

    #[test]
    fn session_new_error_preserves_non_authentication_code() {
        let error = acp_method_error(
            AGENT_METHOD_NAMES.session_new,
            agent_client_protocol::Error::internal_error(),
        );

        assert_eq!(acp_error_code(&error), Some(ErrorCode::InternalError));
    }

    #[tokio::test]
    async fn prompt_error_update_preserves_auth_required_code() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(AcpUpdate::Error(
            agent_client_protocol::Error::auth_required().data("sign in"),
        ))
        .await
        .unwrap();

        let AcpUpdate::Error(error) = rx.recv().await.unwrap() else {
            panic!("expected ACP error update");
        };
        assert_eq!(error.code, ErrorCode::AuthRequired);
        assert_eq!(error.data, Some(serde_json::json!("sign in")));
    }

    #[test]
    fn prompt_auth_error_maps_to_provider_authentication() {
        let error = provider_error_from_acp(agent_client_protocol::Error::auth_required());

        assert!(matches!(error, ProviderError::Authentication(_)));
    }

    #[test]
    fn prompt_internal_error_remains_request_failed() {
        let error = provider_error_from_acp(agent_client_protocol::Error::internal_error());

        assert!(matches!(error, ProviderError::RequestFailed(_)));
    }

    fn test_provider() -> (AcpProvider, ModelConfig) {
        test_provider_with_tx(None)
    }

    fn test_provider_with_tx(
        tx: Option<mpsc::Sender<ClientRequest>>,
    ) -> (AcpProvider, ModelConfig) {
        (
            AcpProvider {
                name: "acp-test".to_string(),
                goose_mode: Arc::new(Mutex::new(GooseMode::Auto)),
                mode_mapping: HashMap::new(),
                session: Mutex::new(AcpSession {
                    id: SessionId::new("test-session"),
                    response: NewSessionResponse::new("test-session"),
                }),
                pending_confirmations: Arc::new(TokioMutex::new(HashMap::new())),
                pending_tool_updates: Arc::new(Mutex::new(HashMap::new())),
                handoff_context_sent: Arc::new(AtomicBool::new(false)),
                context_size: Arc::new(AtomicU64::new(0)),
                model_config_option_id: None,
                applied_model: Arc::new(Mutex::new(None)),
                effort: AcpEffortState::new(),
                tx,
                cancel_tx: None,
                loop_thread: None,
            },
            ModelConfig::new("test-model"),
        )
    }

    #[tokio::test]
    async fn messages_to_prompt_without_prior_history_preserves_current_prompt() {
        let messages = vec![Message::user().with_text("current request")];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 1);
        assert_eq!(prompt_text(&blocks[0]), "current request");
    }

    #[tokio::test]
    async fn messages_to_prompt_prepends_handoff_context_before_latest_user() {
        let messages = vec![
            Message::user().with_text("inspect src/lib.rs"),
            Message::assistant()
                .with_text("I found the file")
                .with_tool_request("call-1", Ok(CallToolRequestParams::new("read_file"))),
            Message::user().with_tool_response(
                "call-1",
                Ok(CallToolResult::success(vec![RmcpContent::text(
                    "file contents",
                )])),
            ),
            Message::user().with_text("continue from there"),
        ];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 2);
        let memo = prompt_text(&blocks[0]);
        assert!(memo.starts_with(
            "Conversation context from goose before this ACP provider session was created:"
        ));
        assert!(memo.contains("[user]: inspect src/lib.rs"));
        assert!(memo.contains("[assistant]: I found the file"));
        assert!(memo.contains("tool_request(read_file):"));
        assert!(memo.contains("tool_response: file contents"));
        assert!(memo.contains("Current user request follows."));
        assert_eq!(prompt_text(&blocks[1]), "continue from there");
    }

    #[tokio::test]
    async fn messages_to_prompt_skips_turn_context_events() {
        use crate::conversation::message::MessageMetadata;

        let turn_context = |text: &str| {
            Message::user()
                .with_text(text)
                .with_metadata(MessageMetadata::agent_only().with_turn_context())
        };
        let messages = vec![
            Message::user().with_text("inspect src/lib.rs"),
            turn_context("<turn-context>old cwd /repo</turn-context>"),
            Message::assistant().with_text("I found the file"),
            Message::user().with_text("continue from there"),
            turn_context("<turn-context>new cwd /repo</turn-context>"),
        ];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 2);
        assert!(
            !prompt_text(&blocks[0]).contains("turn-context"),
            "handoff memo must not include turn-context events"
        );
        assert_eq!(
            prompt_text(&blocks[1]),
            "continue from there",
            "the current prompt must be the user's request, not a trailing turn-context event"
        );
    }

    #[tokio::test]
    async fn messages_to_prompt_drops_user_only_acp_rows_from_handoff() {
        let user_only = TextContent::new("SECRET_USER_ONLY")
            .annotations(AcpAnnotations::new().audience(vec![AcpRole::User]));
        let messages = vec![
            Message::user().with_text("visible prior"),
            acp_text_update_message(user_only, "acp-message".to_string(), 123),
            Message::user().with_text("current request"),
        ];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 2);
        let memo = prompt_text(&blocks[0]);
        assert!(memo.contains("visible prior"));
        assert!(!memo.contains("SECRET_USER_ONLY"));
        assert!(!memo.contains("<empty message>"));
        assert_eq!(prompt_text(&blocks[1]), "current request");
    }

    #[tokio::test]
    async fn messages_to_prompt_keeps_latest_user_images_after_handoff_memo() {
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user()
                .with_image("base64-image", "image/png")
                .with_text("describe this"),
        ];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 3);
        assert!(prompt_text(&blocks[0]).contains("[assistant]: prior answer"));
        match &blocks[1] {
            ContentBlock::Image(image) => {
                assert_eq!(image.data, "base64-image");
                assert_eq!(image.mime_type, "image/png");
            }
            _ => panic!("expected image block"),
        }
        assert_eq!(prompt_text(&blocks[2]), "describe this");
    }

    #[tokio::test]
    async fn messages_to_prompt_excludes_user_only_current_and_handoff_content() {
        use rmcp::model::{Annotations, TextContent};

        fn user_only_text(text: &str) -> MessageContent {
            MessageContent::Text(
                TextContent::new(text)
                    .with_annotations(Annotations::default().with_audience(vec![Role::User])),
            )
        }

        let messages = vec![
            Message::user()
                .with_text("visible prior")
                .with_content(user_only_text("SECRET_PRIOR")),
            Message::user()
                .with_text("visible current")
                .with_content(user_only_text("SECRET_CURRENT")),
        ];

        let rendered = prompt_with_handoff(&messages)
            .await
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("visible prior"));
        assert!(rendered.contains("visible current"));
        assert!(!rendered.contains("SECRET_PRIOR"));
        assert!(!rendered.contains("SECRET_CURRENT"));
    }

    #[tokio::test]
    async fn messages_to_prompt_drops_handoff_when_current_content_is_user_only() {
        use rmcp::model::{Annotations, TextContent};

        let current = MessageContent::Text(
            TextContent::new("user-only")
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let messages = vec![
            Message::assistant().with_text("prior context"),
            Message::user().with_content(current),
        ];

        assert!(prompt_with_handoff(&messages).await.is_empty());
    }

    #[tokio::test]
    async fn stream_skips_user_only_prompt_without_claiming_handoff_context() {
        use futures::StreamExt;
        use rmcp::model::{Annotations, TextContent};

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let current = MessageContent::Text(
            TextContent::new("user-only")
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let messages = vec![
            Message::assistant().with_text("prior context"),
            Message::user().with_content(current),
        ];

        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();

        assert!(stream.next().await.is_none());
        assert!(rx.try_recv().is_err());
        assert!(!provider.handoff_context_sent.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn chat_mode_denied_tool_messages_get_distinct_provider_ids() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        *provider.goose_mode.lock().unwrap() = GooseMode::Chat;

        let messages = vec![Message::user().with_text("inspect src/lib.rs")];
        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();

        let response_tx = match rx.recv().await.expect("expected ACP prompt request") {
            ClientRequest::Prompt { response_tx, .. } => response_tx,
            _ => panic!("expected ACP prompt request"),
        };

        for id in ["call-1", "call-2"] {
            response_tx
                .send(AcpUpdate::ToolCallStart {
                    id: id.to_string(),
                    name: "read_file".to_string(),
                    kind: ToolKind::Read,
                    raw_input: None,
                })
                .await
                .unwrap();
            response_tx
                .send(AcpUpdate::ToolCallComplete {
                    id: id.to_string(),
                    raw_output: None,
                    content: None,
                    is_error: false,
                })
                .await
                .unwrap();
        }
        response_tx
            .send(AcpUpdate::Complete(StopReason::EndTurn, None))
            .await
            .unwrap();

        let mut messages = Vec::new();
        while let Some(item) = stream.next().await {
            let (message, usage) = item.unwrap();
            assert!(usage.is_none());
            if let Some(message) = message {
                messages.push(message);
            }
        }

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].as_concat_text(), "Tool call was denied.");
        assert_eq!(messages[1].as_concat_text(), "Tool call was denied.");

        let first_id = messages[0]
            .id
            .as_deref()
            .expect("first denial should have a provider message ID");
        let second_id = messages[1]
            .id
            .as_deref()
            .expect("second denial should have a provider message ID");

        assert!(first_id.starts_with("msg_"));
        assert!(second_id.starts_with("msg_"));
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn live_acp_text_update_preserves_assistant_only_audience() {
        let text = TextContent::new("assistant-only")
            .annotations(AcpAnnotations::new().audience(vec![AcpRole::Assistant]));

        let message = acp_text_update_message(text, "message-id".to_string(), 123);

        let MessageContent::Text(text) = &message.content[0] else {
            panic!("expected text content");
        };
        let audience = text
            .annotations
            .as_ref()
            .and_then(|a| a.audience.as_ref())
            .expect("audience annotation should survive");
        assert!(audience.contains(&Role::Assistant));
        assert!(!audience.contains(&Role::User));
    }

    #[test]
    fn handoff_context_is_sent_only_on_first_provider_prompt() {
        let (provider, _) = test_provider();
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let first_claim = provider.claim_handoff_context(&messages);
        assert!(first_claim.first_prompt);
        assert!(first_claim.include_context);

        let second_claim = provider.claim_handoff_context(&messages);
        assert!(!second_claim.first_prompt);
        assert!(!second_claim.include_context);
    }

    #[test]
    fn first_prompt_without_history_still_marks_handoff_context_sent() {
        let (provider, _) = test_provider();
        let first_prompt = vec![Message::user().with_text("new conversation")];
        let later_prompt_with_history = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let first_claim = provider.claim_handoff_context(&first_prompt);
        assert!(first_claim.first_prompt);
        assert!(!first_claim.include_context);

        let later_claim = provider.claim_handoff_context(&later_prompt_with_history);
        assert!(!later_claim.first_prompt);
        assert!(!later_claim.include_context);
    }

    #[tokio::test]
    async fn resume_replaces_session_and_skips_handoff() {
        let (tx, mut rx) = mpsc::channel(2);
        let (provider, _) = test_provider_with_tx(Some(tx));

        let handle = tokio::spawn(async move {
            provider.resume("saved-session").await.unwrap();
            provider
        });

        let ClientRequest::LoadSession {
            session_id,
            response_tx,
        } = rx.recv().await.expect("expected session/load")
        else {
            panic!("expected session/load");
        };
        assert_eq!(session_id.to_string(), "saved-session");
        response_tx
            .send(Ok(NewSessionResponse::new("saved-session")))
            .unwrap();

        let ClientRequest::CloseSession { session_id } =
            rx.recv().await.expect("expected temporary session close")
        else {
            panic!("expected temporary session close");
        };
        assert_eq!(session_id.to_string(), "test-session");

        let provider = handle.await.unwrap();
        assert_eq!(
            provider.provider_session_id().as_deref(),
            Some("saved-session")
        );

        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let claim = provider.claim_handoff_context(&messages);
        assert!(!claim.first_prompt);
        assert!(!claim.include_context);
    }

    #[tokio::test]
    async fn get_context_limit_surfaces_captured_context_size() {
        let (provider, model) = test_provider();
        assert_eq!(
            provider.get_context_limit(&model.model_name, None).await,
            goose_providers::model::DEFAULT_CONTEXT_LIMIT
        );

        provider.context_size.store(200_000, Ordering::Relaxed);
        assert_eq!(
            provider.get_context_limit(&model.model_name, None).await,
            200_000
        );
    }

    #[tokio::test]
    async fn handoff_budget_honors_global_context_limit_override() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", Some("64"))]);
        let (provider, model) = test_provider();
        provider.context_size.store(200_000, Ordering::Relaxed);
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let current_prompt = vec![ContentBlock::Text(TextContent::new("current request"))];

        assert!(
            provider
                .bounded_handoff_memo(&model, &messages, &current_prompt)
                .await
                .is_none(),
            "the global limit should leave too little room for a handoff memo"
        );
    }

    #[tokio::test]
    async fn streamed_error_after_bare_retry_consumes_handoff_context() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let (next_content_tx, next_content_rx) = oneshot::channel();

        // Serve the first prompt and its memo-free retry like a harness that accepts the
        // request but fails while processing it, then capture the following turn.
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                if let Some(ClientRequest::Prompt { response_tx, .. }) = rx.recv().await {
                    let _ = response_tx
                        .send(AcpUpdate::Error(
                            agent_client_protocol::Error::internal_error().data("prompt too large"),
                        ))
                        .await;
                }
            }
            if let Some(ClientRequest::Prompt {
                content,
                response_tx,
                ..
            }) = rx.recv().await
            {
                let _ = next_content_tx.send(content);
                let _ = response_tx
                    .send(AcpUpdate::Complete(StopReason::EndTurn, None))
                    .await;
            }
        });

        let mut first_stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        let first = first_stream.next().await;
        assert!(
            matches!(first, Some(Err(ProviderError::RequestFailed(_)))),
            "expected streamed error, got {first:?}"
        );

        let mut next_stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        let next_content = next_content_rx.await.unwrap();
        assert_eq!(
            next_content.len(),
            1,
            "a rejected memo must not be rebuilt on the next turn"
        );
        assert_eq!(prompt_text(&next_content[0]), "current request");
        assert!(next_stream.next().await.is_none());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_first_prompt_rolls_back_handoff_context_claim() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let server = tokio::spawn(async move {
            if let Some(ClientRequest::Prompt { response_tx, .. }) = rx.recv().await {
                let _ = response_tx
                    .send(AcpUpdate::Complete(StopReason::Cancelled, None))
                    .await;
            }
        });

        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        assert!(stream.next().await.is_none());
        server.await.unwrap();

        let next_claim = provider.claim_handoff_context(&messages);
        assert!(next_claim.include_context);
    }

    #[tokio::test]
    async fn refused_first_prompt_rolls_back_handoff_context_claim() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let server = tokio::spawn(async move {
            if let Some(ClientRequest::Prompt { response_tx, .. }) = rx.recv().await {
                let _ = response_tx
                    .send(AcpUpdate::Complete(StopReason::Refusal, None))
                    .await;
            }
        });

        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        assert!(stream.next().await.is_none());
        server.await.unwrap();

        let next_claim = provider.claim_handoff_context(&messages);
        assert!(next_claim.include_context);
    }

    #[tokio::test]
    async fn completed_first_prompt_commits_handoff_context_claim() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let server = tokio::spawn(async move {
            if let Some(ClientRequest::Prompt { response_tx, .. }) = rx.recv().await {
                let _ = response_tx
                    .send(AcpUpdate::Complete(StopReason::EndTurn, None))
                    .await;
            }
        });

        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        assert!(stream.next().await.is_none());
        server.await.unwrap();

        let next_claim = provider.claim_handoff_context(&messages);
        assert!(!next_claim.include_context);
    }

    #[tokio::test]
    async fn dropped_first_prompt_stream_rolls_back_handoff_context_claim() {
        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        let request = rx.recv().await;
        assert!(matches!(request, Some(ClientRequest::Prompt { .. })));
        drop(stream);

        let next_claim = provider.claim_handoff_context(&messages);
        assert!(next_claim.include_context);
    }

    #[tokio::test]
    async fn failed_first_prompt_send_without_handoff_rolls_back_claim() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![Message::user().with_text("current request")];

        let result = provider.stream(&model, "", &messages, &[]).await;

        assert!(matches!(result, Err(ProviderError::RequestFailed(_))));
        let next_claim = provider.claim_handoff_context(&messages);
        assert!(next_claim.first_prompt);
    }

    #[tokio::test]
    async fn failed_handoff_send_consumes_the_claim() {
        let _guard = env_lock::lock_env([("GOOSE_CONTEXT_LIMIT", None::<&str>)]);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let result = provider.stream(&model, "", &messages, &[]).await;

        assert!(matches!(result, Err(ProviderError::RequestFailed(_))));
        let next_claim = provider.claim_handoff_context(&messages);
        assert!(
            !next_claim.include_context,
            "a memo that already failed to send must not be rebuilt"
        );
    }

    fn expect_prompt(request: ClientRequest) -> (Vec<ContentBlock>, mpsc::Sender<AcpUpdate>) {
        match request {
            ClientRequest::Prompt {
                content,
                response_tx,
                ..
            } => (content, response_tx),
            _ => panic!("expected ACP prompt request"),
        }
    }

    #[tokio::test]
    async fn rejected_handoff_prompt_retries_once_without_the_memo() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(2);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let handle = tokio::spawn(async move {
            let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
            let mut results = Vec::new();
            while let Some(item) = stream.next().await {
                results.push(item);
            }
            results
        });

        let (content, response_tx) = expect_prompt(rx.recv().await.unwrap());
        assert_eq!(content.len(), 2, "memo precedes the current prompt");
        response_tx
            .send(AcpUpdate::Error(
                agent_client_protocol::Error::invalid_request().data("Prompt is too long"),
            ))
            .await
            .unwrap();

        let (content, response_tx) = expect_prompt(rx.recv().await.unwrap());
        assert_eq!(content.len(), 1, "retry carries the current prompt only");
        assert_eq!(prompt_text(&content[0]), "current request");
        response_tx
            .send(AcpUpdate::Complete(StopReason::EndTurn, None))
            .await
            .unwrap();

        let results = handle.await.unwrap();
        assert!(results.iter().all(|item| item.is_ok()));
    }

    #[tokio::test]
    async fn rejected_retry_surfaces_the_error() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(2);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let handle = tokio::spawn(async move {
            let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
            let mut results = Vec::new();
            while let Some(item) = stream.next().await {
                results.push(item);
            }
            results
        });

        for _ in 0..2 {
            let (_, response_tx) = expect_prompt(rx.recv().await.unwrap());
            response_tx
                .send(AcpUpdate::Error(
                    agent_client_protocol::Error::invalid_request().data("Prompt is too long"),
                ))
                .await
                .unwrap();
        }

        let results = handle.await.unwrap();
        assert!(matches!(
            results.as_slice(),
            [Err(ProviderError::RequestFailed(_))]
        ));
        assert!(rx.try_recv().is_err(), "exactly one retry");
    }

    #[tokio::test]
    async fn auth_failure_surfaces_instead_of_retrying_without_the_memo() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(2);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let handle = tokio::spawn(async move {
            let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
            let mut results = Vec::new();
            while let Some(item) = stream.next().await {
                results.push(item);
            }
            results
        });

        let (_, response_tx) = expect_prompt(rx.recv().await.unwrap());
        response_tx
            .send(AcpUpdate::Error(
                agent_client_protocol::Error::auth_required(),
            ))
            .await
            .unwrap();

        // As above: a retry would leave the stream waiting on a prompt nothing serves.
        let results = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("stream ended without retrying")
            .unwrap();
        assert!(matches!(
            results.as_slice(),
            [Err(ProviderError::Authentication(_))]
        ));
        assert!(rx.try_recv().is_err(), "no retry on an auth failure");
    }

    #[tokio::test]
    async fn a_budget_too_small_for_a_memo_keeps_the_claim() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(1);
        let (provider, model) = test_provider_with_tx(Some(tx));
        // A window this small leaves no room for a memo beside the current prompt.
        provider.context_size.store(64, Ordering::Relaxed);
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        let server = tokio::spawn(async move {
            let Some(ClientRequest::Prompt {
                content,
                response_tx,
                ..
            }) = rx.recv().await
            else {
                return Vec::new();
            };
            let _ = response_tx
                .send(AcpUpdate::Complete(StopReason::EndTurn, None))
                .await;
            content
        });

        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        assert!(stream.next().await.is_none());
        let content = server.await.unwrap();
        assert_eq!(content.len(), 1, "no memo fit beside the prompt");

        assert!(
            provider.claim_handoff_context(&messages).include_context,
            "context that never left goose must still be handed off later"
        );
    }

    #[tokio::test]
    async fn exhausted_credits_surface_without_spending_the_handoff() {
        use futures::StreamExt;

        let (tx, mut rx) = mpsc::channel(2);
        let (provider, model) = test_provider_with_tx(Some(tx));
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];

        let handle = tokio::spawn(async move {
            let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
            let mut results = Vec::new();
            while let Some(item) = stream.next().await {
                results.push(item);
            }
            (provider, results)
        });

        let (_, response_tx) = expect_prompt(rx.recv().await.unwrap());
        response_tx
            .send(AcpUpdate::Error(
                agent_client_protocol::Error::internal_error()
                    .data(serde_json::json!({ "reason": crate::acp::CREDITS_EXHAUSTED_REASON })),
            ))
            .await
            .unwrap();

        // A retry here would leave the stream waiting on a prompt nothing serves, so bound
        // the wait rather than hanging the suite on a regression.
        let (provider, results) = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("stream ended without retrying")
            .unwrap();
        assert!(matches!(
            results.as_slice(),
            [Err(ProviderError::RequestFailed(_))]
        ));
        assert!(
            rx.try_recv().is_err(),
            "a spent account is not a prompt the agent refused"
        );
        let messages = vec![
            Message::assistant().with_text("prior answer"),
            Message::user().with_text("current request"),
        ];
        assert!(
            provider.claim_handoff_context(&messages).include_context,
            "the memo survives so a topped-up account still resumes the conversation"
        );
    }

    fn test_provider_with_model_option(
        tx: mpsc::Sender<ClientRequest>,
        applied_model: Option<String>,
    ) -> AcpProvider {
        let (mut provider, _) = test_provider_with_tx(Some(tx));
        provider.model_config_option_id = Some("model".to_string());
        provider.applied_model = Arc::new(Mutex::new(applied_model));
        provider
    }

    #[tokio::test]
    async fn apply_model_if_changed_sends_set_config_option_on_change() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_model_option(tx, Some("old-model".to_string()));

        let handle =
            tokio::spawn(async move { provider.apply_model_if_changed("new-model").await });

        match rx.recv().await.expect("expected a SetConfigOption request") {
            ClientRequest::SetConfigOption {
                config_id,
                value,
                response_tx,
                ..
            } => {
                assert_eq!(config_id, "model");
                assert_eq!(value, "new-model");
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("unexpected request kind"),
        }

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn apply_model_if_changed_skips_when_model_unchanged() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_model_option(tx, Some("same-model".to_string()));

        provider.apply_model_if_changed("same-model").await.unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn apply_model_if_changed_noop_without_option_id() {
        let (tx, mut rx) = mpsc::channel(1);
        let (provider, _) = test_provider_with_tx(Some(tx));

        provider.apply_model_if_changed("any-model").await.unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn apply_model_if_changed_skips_sentinel_model() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_model_option(tx, None);

        provider
            .apply_model_if_changed(ACP_CURRENT_MODEL)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    fn effort_select_options(values: &[&str]) -> Vec<SessionConfigSelectOption> {
        values
            .iter()
            .map(|value| SessionConfigSelectOption::new(value.to_string(), *value))
            .collect()
    }

    fn effort_capability(values: &[&str], current: &str) -> ThinkingEffortCapability {
        ThinkingEffortCapability {
            option_id: EFFORT_CONFIG_OPTION_ID.to_string(),
            values: values
                .iter()
                .map(|value| ThinkingEffortOption {
                    value: value.to_string(),
                    label: value.to_string(),
                })
                .collect(),
            current: Some(current.to_string()),
        }
    }

    fn test_provider_with_effort(
        tx: mpsc::Sender<ClientRequest>,
        capability: Option<ThinkingEffortCapability>,
    ) -> AcpProvider {
        let (provider, _) = test_provider_with_tx(Some(tx));
        *provider.effort.capability.lock().unwrap() = capability;
        provider
    }

    fn model_with_effort(value: &str) -> ModelConfig {
        ModelConfig::new(ACP_CURRENT_MODEL).with_merged_request_params(HashMap::from([(
            THINKING_EFFORT_PARAM.to_string(),
            serde_json::json!(value),
        )]))
    }

    async fn expect_set_config_option(rx: &mut mpsc::Receiver<ClientRequest>) -> (String, String) {
        match rx.recv().await.expect("expected a SetConfigOption request") {
            ClientRequest::SetConfigOption {
                config_id,
                value,
                response_tx,
                ..
            } => {
                let _ = response_tx.send(Ok(()));
                (config_id, value)
            }
            _ => panic!("unexpected request kind"),
        }
    }

    async fn fail_set_config_option(
        rx: &mut mpsc::Receiver<ClientRequest>,
        error: agent_client_protocol::Error,
    ) {
        match rx.recv().await.expect("expected a SetConfigOption request") {
            ClientRequest::SetConfigOption { response_tx, .. } => {
                let _ = response_tx.send(Err(anyhow::Error::from(error)));
            }
            _ => panic!("unexpected request kind"),
        }
    }

    #[test]
    fn extract_effort_capability_reads_thought_level_option() {
        let options = vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "sonnet",
                effort_select_options(&["sonnet"]),
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "effort",
                "Thinking",
                "high",
                effort_select_options(&["default", "low", "high", "xhigh"]),
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ];

        let capability =
            extract_effort_capability(&options).expect("effort option should be found");

        assert_eq!(capability.option_id, "effort");
        assert_eq!(capability.current.as_deref(), Some("high"));
        assert_eq!(
            capability
                .values
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["default", "low", "high", "xhigh"]
        );
    }

    #[test]
    fn extract_effort_capability_falls_back_to_effort_option_id() {
        let options = vec![SessionConfigOption::select(
            "effort",
            "Reasoning",
            "low",
            effort_select_options(&["low", "high"]),
        )];

        let capability =
            extract_effort_capability(&options).expect("effort option should be found");

        assert_eq!(capability.option_id, "effort");
        assert_eq!(capability.current.as_deref(), Some("low"));
    }

    #[test]
    fn extract_effort_capability_keeps_agent_labels() {
        let options = vec![SessionConfigOption::select(
            "effort",
            "Thinking",
            "default",
            vec![
                SessionConfigSelectOption::new("default", "Default"),
                SessionConfigSelectOption::new("xhigh", "Extra high"),
            ],
        )
        .category(SessionConfigOptionCategory::ThoughtLevel)];

        let capability =
            extract_effort_capability(&options).expect("effort option should be found");

        assert_eq!(
            capability
                .values
                .iter()
                .map(|option| (option.value.as_str(), option.label.as_str()))
                .collect::<Vec<_>>(),
            vec![("default", "Default"), ("xhigh", "Extra high")]
        );
    }

    #[test]
    fn extract_effort_capability_flattens_grouped_values() {
        let options = vec![SessionConfigOption::select(
            "effort",
            "Thinking",
            "medium",
            vec![
                SessionConfigSelectGroup::new(
                    "standard",
                    "Standard",
                    effort_select_options(&["low", "medium"]),
                ),
                SessionConfigSelectGroup::new(
                    "extended",
                    "Extended",
                    effort_select_options(&["high", "max"]),
                ),
            ],
        )
        .category(SessionConfigOptionCategory::ThoughtLevel)];

        let capability =
            extract_effort_capability(&options).expect("effort option should be found");

        assert_eq!(
            capability
                .values
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "max"]
        );
    }

    #[test]
    fn extract_effort_capability_returns_none_without_effort_option() {
        let options = vec![SessionConfigOption::select(
            "model",
            "Model",
            "sonnet",
            effort_select_options(&["sonnet"]),
        )
        .category(SessionConfigOptionCategory::Model)];

        assert!(extract_effort_capability(&options).is_none());
    }

    #[test]
    fn config_option_update_replaces_effort_state() {
        let effort_state = Arc::new(Mutex::new(Some(effort_capability(&["low"], "low"))));
        let update = ConfigOptionUpdate::new(vec![SessionConfigOption::select(
            "effort",
            "Thinking",
            "xhigh",
            effort_select_options(&["default", "xhigh"]),
        )
        .category(SessionConfigOptionCategory::ThoughtLevel)]);

        refresh_effort_state(&effort_state, &update.config_options);

        let state = effort_state
            .lock()
            .unwrap()
            .clone()
            .expect("state replaced");
        assert_eq!(state.current.as_deref(), Some("xhigh"));
        assert_eq!(
            state
                .values
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            vec!["default", "xhigh"]
        );
    }

    #[test]
    fn refresh_effort_state_clears_when_agent_drops_the_option() {
        let effort_state = Arc::new(Mutex::new(Some(effort_capability(&["low", "high"], "low"))));

        refresh_effort_state(
            &effort_state,
            &[SessionConfigOption::select(
                "model",
                "Model",
                "sonnet",
                effort_select_options(&["sonnet"]),
            )
            .category(SessionConfigOptionCategory::Model)],
        );

        assert!(effort_state.lock().unwrap().is_none());
    }

    #[test]
    fn publish_effort_state_notifies_subscribers() {
        let effort = AcpEffortState::new();
        let mut subscriber = effort.updates.subscribe();

        publish_effort_state(
            &effort,
            &[SessionConfigOption::select(
                "effort",
                "Thinking",
                "high",
                effort_select_options(&["default", "high"]),
            )
            .category(SessionConfigOptionCategory::ThoughtLevel)],
        );

        assert!(subscriber.has_changed().unwrap());
        assert_eq!(
            subscriber.borrow_and_update().clone(),
            ThinkingEffortSupport::Options(effort_capability(&["default", "high"], "high"))
        );
    }

    #[test]
    fn clearing_effort_state_allows_same_options_to_be_republished() {
        let effort = AcpEffortState::new();
        let mut subscriber = effort.updates.subscribe();
        let capability = effort_capability(&["default", "high"], "high");

        replace_effort_state(&effort, Some(capability.clone()));
        assert!(subscriber.has_changed().unwrap());
        subscriber.borrow_and_update();

        assert_eq!(
            replace_effort_state(&effort, None),
            Some(capability.clone())
        );
        assert!(subscriber.has_changed().unwrap());
        assert_eq!(
            subscriber.borrow_and_update().clone(),
            ThinkingEffortSupport::Unsupported
        );

        replace_effort_state(&effort, Some(capability.clone()));
        assert!(subscriber.has_changed().unwrap());
        assert_eq!(
            subscriber.borrow_and_update().clone(),
            ThinkingEffortSupport::Options(capability)
        );
    }

    #[test_case("high" => Some("high".to_string()) ; "exact match passes through")]
    #[test_case("HIGH" => Some("high".to_string()) ; "match ignores case")]
    #[test_case("off" => Some("default".to_string()) ; "off maps to the agent default")]
    #[test_case("max" => Some("xhigh".to_string()) ; "max maps to xhigh")]
    #[test_case("medium" => None ; "value the agent does not offer is not forwarded")]
    fn map_effort_value_maps_goose_values_onto_agent_values(value: &str) -> Option<String> {
        map_effort_value(
            &effort_capability(&["default", "low", "high", "xhigh"], "default"),
            value,
        )
    }

    #[test]
    fn map_effort_value_prefers_max_when_agent_offers_it() {
        let capability = effort_capability(&["default", "max", "xhigh"], "default");

        assert_eq!(
            map_effort_value(&capability, "max"),
            Some("max".to_string())
        );
        assert_eq!(
            map_effort_value(&capability, "xhigh"),
            Some("xhigh".to_string())
        );
    }

    #[test]
    fn thinking_effort_support_reports_agent_options() {
        let (tx, _rx) = mpsc::channel(1);
        let capability = effort_capability(&["default", "high"], "default");
        let provider = test_provider_with_effort(tx, Some(capability.clone()));

        assert_eq!(
            provider.thinking_effort_support(),
            ThinkingEffortSupport::Options(capability)
        );
    }

    #[test]
    fn thinking_effort_support_is_unsupported_without_an_effort_option() {
        let (provider, _) = test_provider();

        assert_eq!(
            provider.thinking_effort_support(),
            ThinkingEffortSupport::Unsupported
        );
    }

    #[tokio::test]
    async fn set_thinking_effort_forwards_the_pick_to_the_agent() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        let handle = tokio::spawn(async move {
            provider
                .set_thinking_effort("session", "high")
                .await
                .unwrap()
        });

        assert_eq!(
            expect_set_config_option(&mut rx).await,
            ("effort".to_string(), "high".to_string())
        );

        assert!(handle.await.unwrap());
    }

    #[tokio::test]
    async fn set_thinking_effort_is_applied_as_a_no_op_without_an_effort_option() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_effort(tx, None);

        assert!(provider
            .set_thinking_effort("session", "off")
            .await
            .unwrap());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn set_thinking_effort_rejects_non_off_value_without_an_effort_option() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_effort(tx, None);

        let result = provider.set_thinking_effort("session", "high").await;

        assert!(matches!(result, Err(ProviderError::InvalidValue(_))));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn set_thinking_effort_rejects_values_the_agent_does_not_offer() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        let result = provider.set_thinking_effort("session", "medium").await;

        assert!(matches!(result, Err(ProviderError::InvalidValue(_))));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn set_thinking_effort_reports_an_agent_value_rejection_as_invalid() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        let handle =
            tokio::spawn(async move { provider.set_thinking_effort("session", "high").await });
        fail_set_config_option(&mut rx, agent_client_protocol::Error::invalid_params()).await;

        assert!(matches!(
            handle.await.unwrap(),
            Err(ProviderError::InvalidValue(_))
        ));
    }

    /// A dead subprocess or dropped connection surfaces as `internal_error` from
    /// the client library, which says nothing about the value the client picked.
    #[tokio::test]
    async fn set_thinking_effort_reports_a_transport_failure_as_a_request_failure() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        let handle =
            tokio::spawn(async move { provider.set_thinking_effort("session", "high").await });
        fail_set_config_option(&mut rx, agent_client_protocol::Error::internal_error()).await;

        assert!(matches!(
            handle.await.unwrap(),
            Err(ProviderError::RequestFailed(_))
        ));
    }

    #[tokio::test]
    async fn set_thinking_effort_reports_a_dropped_response_as_a_request_failure() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        let handle =
            tokio::spawn(async move { provider.set_thinking_effort("session", "high").await });
        drop(rx.recv().await.expect("expected a SetConfigOption request"));

        assert!(matches!(
            handle.await.unwrap(),
            Err(ProviderError::RequestFailed(_))
        ));
    }

    #[tokio::test]
    async fn apply_effort_if_changed_forwards_the_persisted_value() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_effort(
            tx,
            Some(effort_capability(&["default", "low", "xhigh"], "default")),
        );
        let model = model_with_effort("max");

        let handle =
            tokio::spawn(async move { provider.apply_effort_if_changed(&model).await.unwrap() });

        assert_eq!(
            expect_set_config_option(&mut rx).await,
            ("effort".to_string(), "xhigh".to_string())
        );

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn apply_effort_if_changed_skips_when_agent_already_has_the_value() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "high")));

        provider
            .apply_effort_if_changed(&model_with_effort("high"))
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    /// A refresh that reverts the agent's own effort (e.g. a model switch
    /// rebuilding per-model levels) must not be masked by a stale record of
    /// what goose last sent: the persisted value gets re-applied.
    #[tokio::test]
    async fn apply_effort_if_changed_resends_after_the_agent_resets_its_effort() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "high")));
        refresh_effort_state(
            &provider.effort.capability,
            &[SessionConfigOption::select(
                "effort",
                "Thinking",
                "default",
                effort_select_options(&["default", "high"]),
            )
            .category(SessionConfigOptionCategory::ThoughtLevel)],
        );

        let handle = tokio::spawn(async move {
            provider
                .apply_effort_if_changed(&model_with_effort("high"))
                .await
                .unwrap()
        });

        assert_eq!(
            expect_set_config_option(&mut rx).await,
            ("effort".to_string(), "high".to_string())
        );

        handle.await.unwrap();
    }

    /// The menu advertises the global default once the persisted pick stops
    /// being offered (a model switch rebuilt the agent's selector), so the send
    /// path has to apply it too — otherwise the agent silently runs at its own
    /// current while the client shows the global.
    #[tokio::test]
    async fn apply_effort_if_changed_falls_back_to_the_global_default() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("high"))]);
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));
        let model = model_with_effort("medium");

        let handle =
            tokio::spawn(async move { provider.apply_effort_if_changed(&model).await.unwrap() });

        assert_eq!(
            expect_set_config_option(&mut rx).await,
            ("effort".to_string(), "high".to_string())
        );

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn apply_effort_if_changed_skips_unmapped_value() {
        // Unoffered by the capability below, so the global default never wins
        // and the test doesn't depend on the machine's configured value.
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("medium"))]);
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        provider
            .apply_effort_if_changed(&model_with_effort("medium"))
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn apply_effort_if_changed_skips_without_an_effort_option() {
        let (tx, mut rx) = mpsc::channel(1);
        let provider = test_provider_with_effort(tx, None);

        provider
            .apply_effort_if_changed(&model_with_effort("high"))
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn apply_effort_if_changed_skips_session_without_a_persisted_value() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("medium"))]);
        let (tx, mut rx) = mpsc::channel(1);
        let provider =
            test_provider_with_effort(tx, Some(effort_capability(&["default", "high"], "default")));

        provider
            .apply_effort_if_changed(&ModelConfig::new(ACP_CURRENT_MODEL))
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    fn test_acp_config(
        mode_mapping: HashMap<GooseMode, Vec<String>>,
        session_mode_id: Option<String>,
    ) -> AcpProviderConfig {
        AcpProviderConfig {
            command: PathBuf::new(),
            args: vec![],
            env: vec![],
            env_remove: vec![],
            work_dir: PathBuf::new(),
            mcp_servers: vec![],
            session_mode_id,
            session_config_options: vec![],
            model_config_option_id: None,
            mode_mapping,
            notification_callback: None,
        }
    }

    #[tokio::test]
    async fn cancelling_start_stops_client_loop() {
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_in_loop = stopped.clone();
        let start = AcpProvider::start(
            "acp-test".to_string(),
            GooseMode::Auto,
            test_acp_config(HashMap::new(), None),
            Box::new(move |_, _, init_tx, cancel_rx| {
                Box::pin(async move {
                    let _init_tx = init_tx;
                    let _ = cancel_rx.await;
                    stopped_in_loop.store(true, Ordering::SeqCst);
                })
            }),
        );

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), start)
                .await
                .is_err()
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !stopped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test_case(GooseMode::Auto)]
    #[test_case(GooseMode::Approve)]
    #[test_case(GooseMode::SmartApprove)]
    #[test_case(GooseMode::Chat)]
    fn initial_mode_candidates_empty_when_mode_negotiation_disabled(mode: GooseMode) {
        let config = test_acp_config(HashMap::new(), None);
        assert!(initial_mode_candidates(&config, Some(mode)).is_empty());
    }

    #[test]
    fn initial_mode_candidates_prefer_mapping_then_fallback() {
        let mapping = HashMap::from([(GooseMode::Auto, vec!["bypassPermissions".to_string()])]);
        let config = test_acp_config(mapping, Some("default".to_string()));

        assert_eq!(
            initial_mode_candidates(&config, Some(GooseMode::Auto)),
            vec!["bypassPermissions".to_string()]
        );
        assert_eq!(
            initial_mode_candidates(&config, Some(GooseMode::Chat)),
            vec!["default".to_string()]
        );
    }

    fn mode_state(current: &str, available: &[&str]) -> SessionModeState {
        SessionModeState::new(
            SessionModeId::new(current),
            available
                .iter()
                .map(|id| SessionMode::new(SessionModeId::new(*id), *id))
                .collect(),
        )
    }

    #[test_case(
        &["full-access", "agent-full-access"],
        &["read-only", "auto", "full-access"],
        Some("full-access")
        ; "zed era ids"
    )]
    #[test_case(
        &["full-access", "agent-full-access"],
        &["read-only", "agent", "agent-full-access"],
        Some("agent-full-access")
        ; "agentclientprotocol era ids"
    )]
    #[test_case(
        &["full-access", "agent-full-access"],
        &["something-else"],
        None
        ; "no candidate offered"
    )]
    fn select_mode_id_picks_first_offered_candidate(
        candidates: &[&str],
        available: &[&str],
        expected: Option<&str>,
    ) {
        let candidates: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
        let modes = mode_state(available[0], available);
        assert_eq!(
            select_mode_id(&candidates, Some(&modes)),
            expected.map(|s| s.to_string())
        );
    }

    #[test]
    fn select_mode_id_first_candidate_when_agent_has_no_modes() {
        let candidates = vec!["full-access".to_string(), "agent-full-access".to_string()];
        assert_eq!(
            select_mode_id(&candidates, None),
            Some("full-access".to_string())
        );
    }

    #[tokio::test]
    async fn update_mode_without_mapping_skips_acp_request_but_tracks_mode() {
        let (tx, mut rx) = mpsc::channel(1);
        let (provider, _) = test_provider_with_tx(Some(tx));

        provider
            .update_mode("session", GooseMode::Chat)
            .await
            .unwrap();

        assert!(rx.try_recv().is_err());
        assert_eq!(*provider.goose_mode.lock().unwrap(), GooseMode::Chat);
    }

    #[tokio::test]
    async fn update_mode_with_mapping_sends_set_mode() {
        let (tx, mut rx) = mpsc::channel(1);
        let (mut provider, _) = test_provider_with_tx(Some(tx));
        provider.mode_mapping = HashMap::from([(GooseMode::Chat, vec!["plan".to_string()])]);

        let handle = tokio::spawn(async move {
            provider
                .update_mode("session", GooseMode::Chat)
                .await
                .unwrap();
            provider
        });

        match rx.recv().await.expect("expected a SetMode request") {
            ClientRequest::SetMode {
                mode_id,
                response_tx,
                ..
            } => {
                assert_eq!(mode_id, "plan");
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("unexpected request kind"),
        }

        let provider = handle.await.unwrap();
        assert_eq!(*provider.goose_mode.lock().unwrap(), GooseMode::Chat);
    }

    #[tokio::test]
    async fn update_mode_sends_candidate_offered_by_agent() {
        let (tx, mut rx) = mpsc::channel(1);
        let (mut provider, _) = test_provider_with_tx(Some(tx));
        provider.mode_mapping = HashMap::from([(
            GooseMode::Auto,
            vec!["full-access".to_string(), "agent-full-access".to_string()],
        )]);
        provider.session.lock().unwrap().response = NewSessionResponse::new("test-session").modes(
            mode_state("read-only", &["read-only", "agent", "agent-full-access"]),
        );

        let handle = tokio::spawn(async move {
            provider
                .update_mode("session", GooseMode::Auto)
                .await
                .unwrap();
            provider
        });

        match rx.recv().await.expect("expected a SetMode request") {
            ClientRequest::SetMode {
                mode_id,
                response_tx,
                ..
            } => {
                assert_eq!(mode_id, "agent-full-access");
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("unexpected request kind"),
        }

        let provider = handle.await.unwrap();
        assert_eq!(*provider.goose_mode.lock().unwrap(), GooseMode::Auto);
    }

    #[tokio::test]
    async fn update_mode_errors_when_no_candidate_offered() {
        let (tx, mut rx) = mpsc::channel(1);
        let (mut provider, _) = test_provider_with_tx(Some(tx));
        provider.mode_mapping = HashMap::from([(GooseMode::Chat, vec!["read-only".to_string()])]);
        provider.session.lock().unwrap().response = NewSessionResponse::new("test-session")
            .modes(mode_state("agent", &["agent", "agent-full-access"]));

        let result = provider.update_mode("session", GooseMode::Chat).await;

        assert!(result.is_err());
        assert!(rx.try_recv().is_err());
        assert_eq!(*provider.goose_mode.lock().unwrap(), GooseMode::Auto);
    }

    #[tokio::test]
    async fn messages_to_prompt_includes_all_prior_handoff_context() {
        let messages = vec![
            Message::user().with_text("older context that should be retained"),
            Message::assistant().with_text("middle context"),
            Message::assistant().with_text("recent context"),
            Message::user().with_text("current request"),
        ];

        let blocks = prompt_with_handoff(&messages).await;

        assert_eq!(blocks.len(), 2);
        let memo = prompt_text(&blocks[0]);
        assert!(memo.contains("[user]: older context that should be retained"));
        assert!(memo.contains("[assistant]: middle context"));
        assert!(memo.contains("[assistant]: recent context"));
        assert_eq!(prompt_text(&blocks[1]), "current request");
    }

    #[test_case(
        ExtensionConfig::Stdio {
            name: "github".into(),
            description: String::new(),
            cmd: "/path/to/github-mcp-server".into(),
            args: vec!["stdio".into()],
            envs: Envs::new([("GITHUB_PERSONAL_ACCESS_TOKEN".into(), "ghp_xxxxxxxxxxxx".into())].into()),
            env_keys: vec![],
            timeout: None,
            cwd: None,
            bundled: Some(false),
            available_tools: vec![],
        },
        vec![
            McpServer::Stdio(
                McpServerStdio::new("github", "/path/to/github-mcp-server")
                    .args(vec!["stdio".into()])
                    .env(vec![EnvVariable::new("GITHUB_PERSONAL_ACCESS_TOKEN", "ghp_xxxxxxxxxxxx")])
            )
        ]
        ; "stdio_converts_to_mcpserver_stdio"
    )]
    #[test_case(
        ExtensionConfig::StreamableHttp {
            name: "github".into(),
            description: String::new(),
            uri: "https://api.githubcopilot.com/mcp/".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([("Authorization".into(), "Bearer ghp_xxxxxxxxxxxx".into())]),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        },
        vec![
            McpServer::Http(
                McpServerHttp::new("github", "https://api.githubcopilot.com/mcp/")
                    .headers(vec![HttpHeader::new("Authorization", "Bearer ghp_xxxxxxxxxxxx")])
            )
        ]
        ; "streamable_http_converts_to_mcpserver_http_when_capable"
    )]
    fn test_extension_configs_to_mcp_servers(config: ExtensionConfig, expected: Vec<McpServer>) {
        let result = extension_configs_to_mcp_servers(&[config]);
        assert_eq!(result.len(), expected.len(), "server count mismatch");
        for (a, e) in result.iter().zip(expected.iter()) {
            match (a, e) {
                (McpServer::Stdio(actual), McpServer::Stdio(expected)) => {
                    assert_eq!(actual.name, expected.name);
                    assert_eq!(actual.command, expected.command);
                    assert_eq!(actual.args, expected.args);
                    assert_eq!(actual.env.len(), expected.env.len());
                }
                (McpServer::Http(actual), McpServer::Http(expected)) => {
                    assert_eq!(actual.name, expected.name);
                    assert_eq!(actual.url, expected.url);
                    assert_eq!(actual.headers.len(), expected.headers.len());
                }
                _ => panic!("server type mismatch"),
            }
        }
    }

    #[test]
    fn socket_backed_http_is_omitted_without_dropping_ordinary_http() {
        let socket_backed = ExtensionConfig::StreamableHttp {
            name: "socket-backed".into(),
            description: String::new(),
            uri: "http://127.0.0.1:4711/mcp".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([("Authorization".into(), "Bearer socket-secret".into())]),
            timeout: None,
            socket: Some("/run/private-mcp.sock".into()),
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        };
        let ordinary_http = ExtensionConfig::StreamableHttp {
            name: "ordinary-http".into(),
            description: String::new(),
            uri: "https://mcp.example.com/".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([("Authorization".into(), "Bearer network-secret".into())]),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        };

        let result = extension_configs_to_mcp_servers(&[socket_backed, ordinary_http]);

        assert_eq!(
            result,
            vec![McpServer::Http(
                McpServerHttp::new("ordinary-http", "https://mcp.example.com/").headers(vec![
                    HttpHeader::new("Authorization", "Bearer network-secret")
                ])
            )]
        );
    }

    #[test]
    fn test_filter_supported_servers_skips_http_without_capability() {
        let config = ExtensionConfig::StreamableHttp {
            name: "github".into(),
            description: String::new(),
            uri: "https://api.githubcopilot.com/mcp/".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([("Authorization".into(), "Bearer ghp_xxxxxxxxxxxx".into())]),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        };

        let servers = extension_configs_to_mcp_servers(&[config]);
        let filtered = filter_supported_servers(&servers, &McpCapabilities::default());
        assert!(filtered.is_empty());
    }

    #[test_case(GooseMode::Auto => Some(PermissionDecision::AllowOnce) ; "auto allows")]
    #[test_case(GooseMode::Chat => Some(PermissionDecision::RejectOnce) ; "chat rejects")]
    #[test_case(GooseMode::Approve => None ; "approve defers")]
    #[test_case(GooseMode::SmartApprove => None ; "smart_approve defers")]
    fn test_permission_decision_from_mode(mode: GooseMode) -> Option<PermissionDecision> {
        permission_decision_from_mode(mode)
    }

    #[test_case(
        HashMap::from([
            (GooseMode::Auto, vec!["yolo".to_string()]),
            (GooseMode::Approve, vec!["default".to_string()]),
            (GooseMode::SmartApprove, vec!["auto_edit".to_string()]),
            (GooseMode::Chat, vec!["plan".to_string()]),
        ]),
        HashMap::from([
            ("yolo".to_string(), vec![GooseMode::Auto]),
            ("default".to_string(), vec![GooseMode::Approve]),
            ("auto_edit".to_string(), vec![GooseMode::SmartApprove]),
            ("plan".to_string(), vec![GooseMode::Chat]),
        ])
        ; "gemini provider mapping"
    )]
    #[test_case(
        HashMap::from([
            (GooseMode::Auto, vec!["bypassPermissions".to_string()]),
            (GooseMode::Approve, vec!["default".to_string()]),
            (GooseMode::SmartApprove, vec!["acceptEdits".to_string()]),
            (GooseMode::Chat, vec!["plan".to_string()]),
        ]),
        HashMap::from([
            ("bypassPermissions".to_string(), vec![GooseMode::Auto]),
            ("default".to_string(), vec![GooseMode::Approve]),
            ("acceptEdits".to_string(), vec![GooseMode::SmartApprove]),
            ("plan".to_string(), vec![GooseMode::Chat]),
        ])
        ; "claude provider mapping"
    )]
    #[test_case(
        HashMap::from([
            (GooseMode::Auto, vec!["full-access".to_string(), "agent-full-access".to_string()]),
            (GooseMode::Approve, vec!["read-only".to_string()]),
            (GooseMode::SmartApprove, vec!["auto".to_string(), "agent".to_string()]),
            (GooseMode::Chat, vec!["read-only".to_string()]),
        ]),
        HashMap::from([
            ("full-access".to_string(), vec![GooseMode::Auto]),
            ("agent-full-access".to_string(), vec![GooseMode::Auto]),
            ("read-only".to_string(), vec![GooseMode::Approve, GooseMode::Chat]),
            ("auto".to_string(), vec![GooseMode::SmartApprove]),
            ("agent".to_string(), vec![GooseMode::SmartApprove]),
        ])
        ; "codex candidates for both bridge generations"
    )]
    fn test_reverse_mode_mapping(
        forward: HashMap<GooseMode, Vec<String>>,
        expected: HashMap<String, Vec<GooseMode>>,
    ) {
        let result = reverse_mode_mapping(&forward);
        assert_eq!(result.len(), expected.len());
        for (key, expected_modes) in &expected {
            let actual = result.get(key).expect("missing key");
            assert_eq!(
                actual.len(),
                expected_modes.len(),
                "length mismatch for key {key}"
            );
            for mode in expected_modes {
                assert!(actual.contains(mode), "missing {mode:?} for key {key}");
            }
        }
    }

    #[test_case(
        NewSessionResponse::new("s1")
            .config_options(vec![
                SessionConfigOption::select("model", "Model", "default", vec![
                    SessionConfigSelectOption::new("default", "Default (recommended)"),
                    SessionConfigSelectOption::new("sonnet", "Sonnet"),
                    SessionConfigSelectOption::new("haiku", "Haiku"),
                ])
                .category(SessionConfigOptionCategory::Model),
            ])
        => Ok(("default".to_string(), vec!["default".to_string(), "sonnet".to_string(), "haiku".to_string()]))
        ; "model is resolved from config_options"
    )]
    #[test_case(
        NewSessionResponse::new("s1")
            .config_options(vec![
                SessionConfigOption::select("model", "Model", "auto-gemini-3", vec![
                    SessionConfigSelectOption::new("auto-gemini-3", "Auto (Gemini 3)"),
                    SessionConfigSelectOption::new("auto-gemini-2.5", "Auto (Gemini 2.5)"),
                    SessionConfigSelectOption::new("gemini-2.5-pro", "gemini-2.5-pro"),
                ])
                .category(SessionConfigOptionCategory::Model),
            ])
        => Ok(("auto-gemini-3".to_string(), vec!["auto-gemini-3".to_string(), "auto-gemini-2.5".to_string(), "gemini-2.5-pro".to_string()]))
        ; "model with multiple options"
    )]
    #[test_case(
        NewSessionResponse::new("s1")
        => Err(ProviderError::RequestFailed(
            "test: agent returned no model config_options".to_string()
        ))
        ; "missing model config_options is an error"
    )]
    fn test_resolve_model_info(
        response: NewSessionResponse,
    ) -> Result<(String, Vec<String>), ProviderError> {
        resolve_model_info("test", &response)
    }

    fn duplicate_read_only_reverse_modes() -> HashMap<String, Vec<GooseMode>> {
        HashMap::from([
            ("full-access".to_string(), vec![GooseMode::Auto]),
            (
                "read-only".to_string(),
                vec![GooseMode::Approve, GooseMode::Chat],
            ),
            ("auto".to_string(), vec![GooseMode::SmartApprove]),
        ])
    }

    #[test_case(
        "full-access", GooseMode::Auto, Some(GooseMode::Auto)
        ; "unique mapping returns the only candidate"
    )]
    #[test_case(
        "read-only", GooseMode::Approve, Some(GooseMode::Approve)
        ; "duplicate prefers current when current is Approve"
    )]
    #[test_case(
        "read-only", GooseMode::Chat, Some(GooseMode::Chat)
        ; "duplicate prefers current when current is Chat"
    )]
    #[test_case(
        "read-only", GooseMode::Auto, Some(GooseMode::Approve)
        ; "duplicate falls back to first when current not in candidates"
    )]
    #[test_case(
        "unknown-id", GooseMode::Auto, None
        ; "unknown mode id returns None"
    )]
    fn test_resolve_mode(mode_id: &str, current: GooseMode, expected: Option<GooseMode>) {
        let reverse_modes = duplicate_read_only_reverse_modes();
        let current = Arc::new(Mutex::new(current));
        let result = resolve_mode(&reverse_modes, mode_id, &current);
        if mode_id == "read-only" && expected == Some(GooseMode::Approve) {
            assert!(result == Some(GooseMode::Approve) || result == Some(GooseMode::Chat));
        } else {
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn acp_tool_call_content_handles_text_diff_terminal_and_image() {
        use agent_client_protocol::schema::v1::{Diff, Terminal, TerminalId, TextContent};

        let diff_block = ToolCallContent::Diff(
            Diff::new(std::path::PathBuf::from("/tmp/file.txt"), "new\n").old_text("old\n"),
        );
        let terminal_block = ToolCallContent::Terminal(Terminal::new(TerminalId::new("term-7")));
        let text_block = ToolCallContent::Content(agent_client_protocol::schema::v1::Content::new(
            ContentBlock::Text(TextContent::new("hello")),
        ));
        let image_block =
            ToolCallContent::Content(agent_client_protocol::schema::v1::Content::new(
                ContentBlock::Image(ImageContent::new("base64data", "image/png")),
            ));

        let out = acp_tool_call_content_to_rmcp(
            Some(vec![text_block, diff_block, terminal_block, image_block]),
            None,
        );

        assert_eq!(out.len(), 3, "terminal blocks produce no output");
        let serialized: Vec<String> = out
            .iter()
            .map(|c| serde_json::to_string(c).unwrap())
            .collect();
        assert!(
            serialized[0].contains("hello"),
            "text block lost: {serialized:?}"
        );
        assert!(
            serialized[1].contains("/tmp/file.txt"),
            "diff path lost: {serialized:?}"
        );
        assert!(
            serialized[1].contains("new"),
            "diff body lost: {serialized:?}"
        );
        assert!(
            serialized[2].contains("base64data"),
            "image data lost: {serialized:?}"
        );
    }

    #[test]
    fn acp_tool_call_terminal_falls_back_to_raw_output() {
        use agent_client_protocol::schema::v1::{Terminal, TerminalId};

        let terminal_block = ToolCallContent::Terminal(Terminal::new(TerminalId::new("term-1")));
        let raw_output = serde_json::Value::String("hello from shell".to_string());

        let out = acp_tool_call_content_to_rmcp(Some(vec![terminal_block]), Some(raw_output));

        assert_eq!(out.len(), 1);
        let text = out[0].as_text().unwrap();
        assert_eq!(text.text, "hello from shell");
        assert_eq!(
            text.annotations
                .as_ref()
                .and_then(|annotations| annotations.priority),
            Some(0.0)
        );
    }

    #[test]
    fn acp_tool_call_content_preserves_audience_annotations() {
        let text_block = ToolCallContent::Content(agent_client_protocol::schema::v1::Content::new(
            ContentBlock::Text(
                TextContent::new("user-only")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::User])),
            ),
        ));
        let image_block = ToolCallContent::Content(
            agent_client_protocol::schema::v1::Content::new(ContentBlock::Image(
                ImageContent::new("base64data", "image/png")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::Assistant])),
            )),
        );

        let out = acp_tool_call_content_to_rmcp(Some(vec![text_block, image_block]), None);

        let text_audience = out[0]
            .as_text()
            .and_then(|t| t.annotations.as_ref())
            .and_then(|a| a.audience.as_ref())
            .expect("text audience annotation should survive");
        assert!(text_audience.contains(&Role::User));
        assert!(!text_audience.contains(&Role::Assistant));
        assert_eq!(
            out[0]
                .as_text()
                .and_then(|text| text.annotations.as_ref())
                .and_then(|annotations| annotations.priority),
            Some(0.0)
        );
        let image_audience = out[1]
            .as_image()
            .and_then(|i| i.annotations.as_ref())
            .and_then(|a| a.audience.as_ref())
            .expect("image audience annotation should survive");
        assert!(image_audience.contains(&Role::Assistant));
        assert!(!image_audience.contains(&Role::User));
        assert_eq!(
            out[1]
                .as_image()
                .and_then(|image| image.annotations.as_ref())
                .and_then(|annotations| annotations.priority),
            Some(0.0)
        );
    }

    #[test]
    fn acp_tool_call_fallback_content_preserves_audience_annotations() {
        use agent_client_protocol::schema::v1::{
            AudioContent, EmbeddedResource, EmbeddedResourceResource, ResourceLink,
            TextResourceContents,
        };

        let blocks = [
            ContentBlock::Audio(
                AudioContent::new("audio-user", "audio/wav")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::User])),
            ),
            ContentBlock::Audio(
                AudioContent::new("audio-assistant", "audio/wav")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::Assistant])),
            ),
            ContentBlock::ResourceLink(
                ResourceLink::new("user resource", "file:///user")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::User])),
            ),
            ContentBlock::ResourceLink(
                ResourceLink::new("assistant resource", "file:///assistant")
                    .annotations(AcpAnnotations::new().audience(vec![AcpRole::Assistant])),
            ),
            ContentBlock::Resource(
                EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new("user body", "file:///user-body"),
                ))
                .annotations(AcpAnnotations::new().audience(vec![AcpRole::User])),
            ),
            ContentBlock::Resource(
                EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new("assistant body", "file:///assistant-body"),
                ))
                .annotations(AcpAnnotations::new().audience(vec![AcpRole::Assistant])),
            ),
        ];
        let content = blocks
            .into_iter()
            .map(|block| {
                ToolCallContent::Content(agent_client_protocol::schema::v1::Content::new(block))
            })
            .collect();

        let out = acp_tool_call_content_to_rmcp(Some(content), None);

        assert_eq!(out.len(), 6);
        for (content, expected_role) in out.iter().zip([
            Role::User,
            Role::Assistant,
            Role::User,
            Role::Assistant,
            Role::User,
            Role::Assistant,
        ]) {
            let annotations = content
                .as_text()
                .and_then(|text| text.annotations.as_ref())
                .expect("fallback content should carry annotations");
            assert_eq!(annotations.audience.as_deref(), Some(&[expected_role][..]));
            assert_eq!(annotations.priority, Some(0.0));
        }
    }

    #[test]
    fn acp_tool_call_content_falls_back_to_raw_output_when_blocks_empty() {
        let out =
            acp_tool_call_content_to_rmcp(Some(vec![]), Some(serde_json::json!({"key": "value"})));
        assert_eq!(out.len(), 1);
        let serialized = serde_json::to_string(&out[0]).unwrap();
        assert!(
            serialized.contains("key"),
            "fallback raw_output lost: {serialized}"
        );
    }

    /// Pins the tool_meta shape that the `AcpUpdate::ToolCallStart` consumer
    /// emits onto the synthesized `ToolRequest`. ACP doesn't expose a canonical
    /// tool name to clients, so we surface `kind` here as a stable categorization
    /// signal alongside the `external_dispatch` marker that bypasses agent-loop
    /// routing.
    #[test]
    fn tool_meta_pairs_external_dispatch_marker_with_acp_kind() {
        let cases = [
            (ToolKind::Execute, "execute"),
            (ToolKind::Read, "read"),
            (ToolKind::Edit, "edit"),
            (ToolKind::Other, "other"),
        ];
        for (kind, expected) in cases {
            let tool_meta = serde_json::json!({
                TOOL_META_EXTERNAL_DISPATCH_KEY: true,
                "goose.acp.kind": kind,
            });
            assert_eq!(
                tool_meta[TOOL_META_EXTERNAL_DISPATCH_KEY],
                serde_json::Value::Bool(true),
                "external_dispatch marker missing for kind={kind:?}"
            );
            assert_eq!(
                tool_meta["goose.acp.kind"],
                serde_json::Value::String(expected.to_string()),
                "goose.acp.kind serialized wrong for kind={kind:?}"
            );
        }
    }
}
