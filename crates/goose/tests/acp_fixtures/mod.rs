#![recursion_limit = "256"]
#![allow(unused_attributes)]

use agent_client_protocol::schema::v1::{
    CreateTerminalResponse, KillTerminalResponse, ListSessionsResponse, McpServer,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalResponse, SessionModeState,
    SessionUpdate, TerminalExitStatus, TerminalId, TerminalOutputResponse, ToolCallContent,
    ToolCallStatus, ToolKind, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fs_err as fs;
use goose::acp::server::{serve, AcpProviderFactory, GooseAcpAgent, GooseAcpAgentOptions};
pub use goose::acp::{map_permission_response, PermissionDecision};
use goose::agents::GoosePlatform;
use goose::builtin_extension::register_builtin_extensions;
use goose::config::paths::Paths;
use goose::config::{GooseMode, PermissionManager};
use goose::providers::api_client::{ApiClient, AuthMethod as ApiAuthMethod};
use goose::providers::base::Provider;
use goose::providers::openai::OpenAiProvider;
use goose::scheduler::{ScheduledJob, SchedulerError, ValidatedScheduleRecipe};
use goose::scheduler_trait::SchedulerTrait;
use goose::session::Session as GooseSession;
use goose::session_context::SESSION_ID_HEADER;
use goose_test_support::{ExpectedSessionId, TEST_MODEL};
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ACP_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static ACP_CONFIG_ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().unwrap());

struct FixtureScheduler {
    jobs: tokio::sync::Mutex<Vec<ScheduledJob>>,
}

impl FixtureScheduler {
    fn new() -> Self {
        Self {
            jobs: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    async fn job_mut<F>(&self, id: &str, update: F) -> Result<(), SchedulerError>
    where
        F: FnOnce(&mut ScheduledJob),
    {
        let mut jobs = self.jobs.lock().await;
        let job = jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| SchedulerError::JobNotFound(id.to_string()))?;
        update(job);
        Ok(())
    }
}

#[async_trait]
impl SchedulerTrait for FixtureScheduler {
    async fn add_scheduled_job(
        &self,
        job: ScheduledJob,
        _copy_recipe: bool,
    ) -> Result<(), SchedulerError> {
        let mut jobs = self.jobs.lock().await;
        if jobs.iter().any(|existing| existing.id == job.id) {
            return Err(SchedulerError::JobIdExists(job.id));
        }
        jobs.push(job);
        Ok(())
    }

    async fn add_scheduled_job_with_recipe(
        &self,
        job: ScheduledJob,
        _validated_recipe: ValidatedScheduleRecipe,
    ) -> Result<(), SchedulerError> {
        self.add_scheduled_job(job, false).await
    }

    async fn schedule_recipe(
        &self,
        recipe_path: PathBuf,
        cron_schedule: Option<String>,
    ) -> anyhow::Result<(), SchedulerError> {
        let id = recipe_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("test_recipe")
            .to_string();
        self.add_scheduled_job(
            ScheduledJob {
                id,
                source: recipe_path.to_string_lossy().to_string(),
                cron: cron_schedule.unwrap_or_else(|| "0 0 * * * *".to_string()),
                last_run: None,
                currently_running: false,
                paused: false,
                current_session_id: None,
                process_start_time: None,
                parameters: Vec::new(),
                recipe_base_dir: recipe_path
                    .parent()
                    .map(|parent| parent.to_string_lossy().to_string()),
            },
            false,
        )
        .await
    }

    async fn list_scheduled_jobs(&self) -> Vec<ScheduledJob> {
        self.jobs.lock().await.clone()
    }

    async fn remove_scheduled_job(
        &self,
        id: &str,
        _remove_recipe: bool,
    ) -> Result<(), SchedulerError> {
        let mut jobs = self.jobs.lock().await;
        if let Some(index) = jobs.iter().position(|job| job.id == id) {
            jobs.remove(index);
            Ok(())
        } else {
            Err(SchedulerError::JobNotFound(id.to_string()))
        }
    }

    async fn pause_schedule(&self, id: &str) -> Result<(), SchedulerError> {
        self.job_mut(id, |job| job.paused = true).await
    }

    async fn unpause_schedule(&self, id: &str) -> Result<(), SchedulerError> {
        self.job_mut(id, |job| job.paused = false).await
    }

    async fn run_now(&self, id: &str) -> Result<String, SchedulerError> {
        self.job_mut(id, |job| {
            job.last_run = Some(Utc::now());
            job.current_session_id = Some("test_session_123".to_string());
        })
        .await?;
        Ok("test_session_123".to_string())
    }

    async fn sessions(
        &self,
        sched_id: &str,
        _limit: usize,
    ) -> Result<Vec<(String, GooseSession)>, SchedulerError> {
        let jobs = self.jobs.lock().await;
        if jobs.iter().any(|job| job.id == sched_id) {
            Ok(Vec::new())
        } else {
            Err(SchedulerError::JobNotFound(sched_id.to_string()))
        }
    }

    async fn update_schedule(
        &self,
        sched_id: &str,
        new_cron: String,
    ) -> Result<(), SchedulerError> {
        self.job_mut(sched_id, |job| job.cron = new_cron).await
    }

    async fn kill_running_job(&self, sched_id: &str) -> Result<(), SchedulerError> {
        self.job_mut(sched_id, |job| {
            job.currently_running = false;
            job.current_session_id = None;
            job.process_start_time = None;
        })
        .await
    }

    async fn get_running_job_info(
        &self,
        sched_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, SchedulerError> {
        let jobs = self.jobs.lock().await;
        let job = jobs
            .iter()
            .find(|job| job.id == sched_id)
            .ok_or_else(|| SchedulerError::JobNotFound(sched_id.to_string()))?;
        Ok(job.current_session_id.clone().zip(job.process_start_time))
    }
}

fn write_global_test_config(config_path: &Path, openai_base_url: &str) {
    let contents = fs::read_to_string(config_path).unwrap();
    let mut config: serde_yaml::Mapping = serde_yaml::from_str(&contents).unwrap();
    config.insert(
        serde_yaml::Value::String("OPENAI_HOST".to_string()),
        serde_yaml::Value::String(openai_base_url.to_string()),
    );

    let global_config_dir = Paths::config_dir();
    fs::create_dir_all(&global_config_dir).unwrap();
    let global_config_path = global_config_dir.join(goose::config::base::CONFIG_YAML_NAME);
    fs::write(&global_config_path, serde_yaml::to_string(&config).unwrap()).unwrap();
}

pub struct OpenAiFixture {
    _server: MockServer,
    base_url: String,
    exchanges: Vec<(String, &'static str)>,
    queue: Arc<Mutex<VecDeque<(String, &'static str)>>>,
}

impl OpenAiFixture {
    /// Mock OpenAI streaming endpoint. Exchanges are (pattern, response) pairs.
    /// On mismatch, returns 417 of the diff in OpenAI error format.
    pub async fn new(
        exchanges: Vec<(String, &'static str)>,
        expected_session_id: Arc<dyn ExpectedSessionId>,
    ) -> Self {
        let mock_server = MockServer::start().await;
        let queue = Arc::new(Mutex::new(VecDeque::from(exchanges.clone())));

        // Always return the models when asked, as there is no POST data to validate
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(include_str!("../acp_test_data/openai_models.json")),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with({
                let queue = queue.clone();
                let expected_session_id = expected_session_id.clone();
                move |req: &wiremock::Request| {
                    let body = std::str::from_utf8(&req.body).unwrap_or("");

                    // Validate session ID header
                    let actual = req
                        .headers
                        .get(SESSION_ID_HEADER)
                        .and_then(|v| v.to_str().ok());
                    if let Err(e) = expected_session_id.validate(actual) {
                        return ResponseTemplate::new(417)
                            .insert_header("content-type", "application/json")
                            .set_body_json(serde_json::json!({"error": {"message": e}}));
                    }

                    // See if the actual request matches the expected pattern
                    let mut q = queue.lock().unwrap();
                    let (expected_body, response) = q.front().cloned().unwrap_or_default();
                    if !expected_body.is_empty() && body.contains(&expected_body) {
                        q.pop_front();
                        return ResponseTemplate::new(200)
                            .insert_header("content-type", "text/event-stream")
                            .set_body_string(response);
                    }
                    drop(q);

                    // If there was no body, the request was unexpected. Otherwise, it is a mismatch.
                    let message = if expected_body.is_empty() {
                        format!("Unexpected request:\n  {}", body)
                    } else {
                        format!(
                            "Expected body to contain:\n  {}\n\nActual body:\n  {}",
                            expected_body, body
                        )
                    };
                    // Use OpenAI's error response schema so the provider will pass the error through.
                    ResponseTemplate::new(417)
                        .insert_header("content-type", "application/json")
                        .set_body_json(serde_json::json!({"error": {"message": message}}))
                }
            })
            .mount(&mock_server)
            .await;

        let base_url = mock_server.uri();
        Self {
            _server: mock_server,
            base_url,
            exchanges,
            queue,
        }
    }

    pub fn uri(&self) -> &str {
        &self.base_url
    }

    pub fn reset(&self) {
        let mut queue = self.queue.lock().unwrap();
        *queue = VecDeque::from(self.exchanges.clone());
    }
}

type CompatDuplexStream = tokio_util::compat::Compat<tokio::io::DuplexStream>;

pub struct DuplexTransport {
    outgoing: CompatDuplexStream,
    incoming: CompatDuplexStream,
}

impl DuplexTransport {
    fn new(outgoing: CompatDuplexStream, incoming: CompatDuplexStream) -> Self {
        Self { outgoing, incoming }
    }

    fn into_byte_streams(
        self,
    ) -> agent_client_protocol::ByteStreams<CompatDuplexStream, CompatDuplexStream> {
        agent_client_protocol::ByteStreams::new(self.outgoing, self.incoming)
    }

    fn into_parts(self) -> (CompatDuplexStream, CompatDuplexStream) {
        (self.outgoing, self.incoming)
    }
}

/// Wires up duplex streams, spawns `serve` for the given agent, and returns
/// the client transport plus the server handle.
#[allow(dead_code)]
pub async fn serve_agent_in_process(
    agent: Arc<GooseAcpAgent>,
) -> (DuplexTransport, JoinHandle<()>) {
    let (client_read, server_write) = tokio::io::duplex(64 * 1024);
    let (server_read, client_write) = tokio::io::duplex(64 * 1024);

    let handle = tokio::spawn(async move {
        if let Err(e) = serve(agent, server_read.compat(), server_write.compat_write()).await {
            tracing::error!("ACP server error: {e}");
        }
    });

    let transport = DuplexTransport::new(client_write.compat_write(), client_read.compat());
    (transport, handle)
}

#[allow(dead_code)]
pub async fn spawn_acp_server_in_process(
    openai_base_url: &str,
    builtins: &[String],
    data_root: &std::path::Path,
    goose_mode: GooseMode,
    provider_factory: Option<AcpProviderFactory>,
    current_model: &str,
    disable_session_naming: bool,
) -> (DuplexTransport, JoinHandle<()>, Arc<PermissionManager>) {
    fs::create_dir_all(data_root).unwrap();
    // TODO: Paths::in_state_dir is global, ignoring per-test data_root
    fs::create_dir_all(Paths::in_state_dir("logs")).unwrap();
    let config_path = data_root.join(goose::config::base::CONFIG_YAML_NAME);
    if !config_path.exists() {
        fs::write(
            &config_path,
            format!(
                "GOOSE_MODEL: {current_model}\nGOOSE_PROVIDER: openai\nGOOSE_MODE: {}\nGOOSE_TOOL_PAIR_SUMMARIZATION: false\n",
                goose_mode
            ),
        )
        .unwrap();
    }
    write_global_test_config(&config_path, openai_base_url);
    let provider_factory = provider_factory.unwrap_or_else(|| {
        let base_url = openai_base_url.to_string();
        Arc::new(
            move |_provider_name, _extensions, _working_dir, _use_default_model| {
                let base_url = base_url.clone();
                Box::pin(async move {
                    let api_client = ApiClient::new_with_tls(
                        base_url,
                        ApiAuthMethod::BearerToken("test-key".to_string()),
                        None,
                    )
                    .unwrap();
                    let provider: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(api_client));
                    Ok(provider)
                })
            },
        )
    });

    let agent = GooseAcpAgent::new(GooseAcpAgentOptions {
        provider_factory,
        builtin_selection: goose::acp::server::AcpBuiltinSelection {
            explicit: builtins.to_vec(),
            ..Default::default()
        },
        data_dir: data_root.to_path_buf(),
        config_dir: data_root.to_path_buf(),
        disable_session_naming,
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        session_cwd: None,
        scheduler: Some(Arc::new(FixtureScheduler::new())),
        active_prompt_runs: Default::default(),
    })
    .await
    .unwrap();
    let agent = Arc::new(agent);
    let permission_manager = agent.permission_manager();
    let (transport, handle) = serve_agent_in_process(agent).await;

    (transport, handle, permission_manager)
}

#[derive(Debug)]
pub struct TestOutput {
    pub text: String,
    pub tool_status: Option<ToolCallStatus>,
}

#[derive(Debug, PartialEq)]
pub enum Notification {
    UserMessage,
    AgentMessage,
    AgentThought,
    ToolCall,
    ToolCallKind(ToolKind),
    ToolCallContent(String),
    ToolCallStatus(ToolCallStatus),
    Plan,
    AvailableCommands,
    CurrentMode,
    ConfigOption,
    SessionInfoUpdate {
        title: Option<String>,
        updated_at: Option<String>,
        message_count: Option<u64>,
        user_set_name: Option<bool>,
    },
}

pub fn to_notifications(updates: &[SessionUpdate]) -> Vec<Notification> {
    let mut out = Vec::new();
    for u in updates {
        match u {
            SessionUpdate::UserMessageChunk(_) => {
                if out.last() != Some(&Notification::UserMessage) {
                    out.push(Notification::UserMessage);
                }
            }
            SessionUpdate::AgentMessageChunk(_) => {
                if out.last() != Some(&Notification::AgentMessage) {
                    out.push(Notification::AgentMessage);
                }
            }
            SessionUpdate::AgentThoughtChunk(_) => {
                if out.last() != Some(&Notification::AgentThought) {
                    out.push(Notification::AgentThought);
                }
            }
            SessionUpdate::ToolCall(_) => out.push(Notification::ToolCall),
            SessionUpdate::ToolCallUpdate(upd) => {
                if let Some(kind) = upd.fields.kind {
                    out.push(Notification::ToolCallKind(kind));
                }
                if let Some(ref content) = upd.fields.content {
                    for c in content {
                        let tag = match c {
                            ToolCallContent::Content(_) => "content",
                            ToolCallContent::Diff(_) => "diff",
                            ToolCallContent::Terminal(_) => "terminal",
                            _ => "unknown",
                        };
                        out.push(Notification::ToolCallContent(tag.into()));
                    }
                }
                if let Some(status) = upd.fields.status {
                    out.push(Notification::ToolCallStatus(status));
                }
            }
            SessionUpdate::Plan(_) => out.push(Notification::Plan),
            SessionUpdate::CurrentModeUpdate(_) => out.push(Notification::CurrentMode),
            SessionUpdate::ConfigOptionUpdate(_) => out.push(Notification::ConfigOption),
            SessionUpdate::SessionInfoUpdate(update) => {
                let meta = update.meta.as_ref();
                let is_active_run_update = meta
                    .and_then(|m| m.get("goose"))
                    .and_then(|g| g.get("activeRunId"))
                    .is_some();
                if is_active_run_update {
                    continue;
                }
                out.push(Notification::SessionInfoUpdate {
                    title: update.title.value().cloned(),
                    updated_at: update.updated_at.value().cloned(),
                    message_count: meta
                        .and_then(|m| m.get("messageCount"))
                        .and_then(|v| v.as_u64()),
                    user_set_name: meta
                        .and_then(|m| m.get("userSetName"))
                        .and_then(|v| v.as_bool()),
                });
            }
            _ => {}
        }
    }
    out
}

pub fn assert_notifications(actual: &[Notification], expected: &[Notification]) {
    assert_eq!(actual, expected);
}

type ReadTextFileHandler =
    Arc<dyn Fn(&ReadTextFileRequest) -> Result<ReadTextFileResponse, String> + Send + Sync>;
type WriteTextFileHandler =
    Arc<dyn Fn(&WriteTextFileRequest) -> Result<WriteTextFileResponse, String> + Send + Sync>;

#[derive(Clone)]
pub struct FsFixture {
    calls: Arc<Mutex<Vec<Result<(), String>>>>,
}

impl FsFixture {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn read_handler(&self, expected_path: &str, content: &str) -> ReadTextFileHandler {
        let calls = self.calls.clone();
        let expected_path = expected_path.to_string();
        let content = content.to_string();
        Arc::new(move |req: &ReadTextFileRequest| {
            let path = req.path.to_str().unwrap_or("");
            if path != expected_path {
                let err = format!("expected path {expected_path}, got {path}");
                calls.lock().unwrap().push(Err(err.clone()));
                return Err(err);
            }
            calls.lock().unwrap().push(Ok(()));
            Ok(ReadTextFileResponse::new(&content))
        })
    }

    pub fn write_handler(
        &self,
        expected_path: &str,
        expected_content: &str,
    ) -> WriteTextFileHandler {
        let calls = self.calls.clone();
        let expected_path = expected_path.to_string();
        let expected_content = expected_content.to_string();
        Arc::new(move |req: &WriteTextFileRequest| {
            let path = req.path.to_str().unwrap_or("");
            if path != expected_path {
                let err = format!("expected path {expected_path}, got {path}");
                calls.lock().unwrap().push(Err(err.clone()));
                return Err(err);
            }
            if req.content != expected_content {
                let err = format!("expected content {expected_content}, got {}", req.content);
                calls.lock().unwrap().push(Err(err.clone()));
                return Err(err);
            }
            calls.lock().unwrap().push(Ok(()));
            Ok(WriteTextFileResponse::new())
        })
    }

    pub fn assert_called(&self) {
        let calls = self.calls.lock().unwrap();
        assert!(!calls.is_empty(), "fs handler was never called");
        let errors: Vec<_> = calls.iter().filter_map(|c| c.as_ref().err()).collect();
        assert!(errors.is_empty(), "fs handler errors: {errors:?}");
    }
}

/// Expected terminal calls. Each variant carries (expected_input, return_value) data,
/// like OpenAiFixture's (pattern, response) pairs.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TerminalCall {
    Create(String, String),      // (command, terminal_id)
    WaitForExit(String, u32),    // (terminal_id, exit_code)
    Output(String, String, u32), // (terminal_id, text, exit_code)
    Release(String),             // terminal_id
    Kill(String),                // terminal_id
}

impl TerminalCall {
    fn name(&self) -> &'static str {
        match self {
            Self::Create(..) => "create",
            Self::WaitForExit(..) => "wait_for_exit",
            Self::Output(..) => "output",
            Self::Release(_) => "release",
            Self::Kill(_) => "kill",
        }
    }
}

pub struct TerminalFixture {
    queue: Arc<Mutex<VecDeque<TerminalCall>>>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl TerminalFixture {
    pub fn new(calls: Vec<TerminalCall>) -> Arc<Self> {
        Arc::new(Self {
            queue: Arc::new(Mutex::new(VecDeque::from(calls))),
            errors: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn pop(&self, expected: &str) -> Option<TerminalCall> {
        let Some(call) = self.queue.lock().unwrap().pop_front() else {
            self.record_error(format!("unexpected {expected}: queue empty"));
            return None;
        };
        if call.name() != expected {
            self.record_error(format!("expected {expected}, got {}", call.name()));
            return None;
        }
        Some(call)
    }

    fn record_error(&self, msg: String) {
        self.errors.lock().unwrap().push(msg);
    }

    fn validate_terminal_id(&self, method: &str, expected: &str, actual: &TerminalId) {
        if expected != actual.0.as_ref() {
            self.record_error(format!(
                "{method}: expected terminal_id {expected}, got {actual}"
            ));
        }
    }

    pub fn on_create(&self, command: &str) -> CreateTerminalResponse {
        if let Some(TerminalCall::Create(expect_command, terminal_id)) = self.pop("create") {
            if command != expect_command {
                self.record_error(format!(
                    "create: expected command {expect_command}, got {command}"
                ));
            }
            CreateTerminalResponse::new(TerminalId::new(terminal_id))
        } else {
            CreateTerminalResponse::new(TerminalId::new("error"))
        }
    }

    pub fn on_wait_for_exit(&self, terminal_id: &TerminalId) -> WaitForTerminalExitResponse {
        if let Some(TerminalCall::WaitForExit(expected_id, exit_code)) = self.pop("wait_for_exit") {
            self.validate_terminal_id("wait_for_exit", &expected_id, terminal_id);
            WaitForTerminalExitResponse::new(TerminalExitStatus::new().exit_code(exit_code))
        } else {
            WaitForTerminalExitResponse::new(TerminalExitStatus::new().exit_code(1))
        }
    }

    pub fn on_output(&self, terminal_id: &TerminalId) -> TerminalOutputResponse {
        if let Some(TerminalCall::Output(expected_id, text, exit_code)) = self.pop("output") {
            self.validate_terminal_id("output", &expected_id, terminal_id);
            TerminalOutputResponse::new(text, false)
                .exit_status(TerminalExitStatus::new().exit_code(exit_code))
        } else {
            TerminalOutputResponse::new("", false)
        }
    }

    pub fn on_release(&self, terminal_id: &TerminalId) -> ReleaseTerminalResponse {
        if let Some(TerminalCall::Release(expected_id)) = self.pop("release") {
            self.validate_terminal_id("release", &expected_id, terminal_id);
        }
        ReleaseTerminalResponse::new()
    }

    pub fn on_kill(&self, terminal_id: &TerminalId) -> KillTerminalResponse {
        if let Some(TerminalCall::Kill(expected_id)) = self.pop("kill") {
            self.validate_terminal_id("kill", &expected_id, terminal_id);
        }
        KillTerminalResponse::new()
    }

    pub fn assert_called(&self) {
        let errors = self.errors.lock().unwrap();
        assert!(errors.is_empty(), "terminal fixture errors: {errors:?}");
        let queue = self.queue.lock().unwrap();
        assert!(
            queue.is_empty(),
            "terminal fixture has unconsumed calls: {queue:?}"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStateFixture {
    pub current_model_id: String,
    pub available_models: Vec<String>,
}

#[derive(Debug)]
pub struct SessionData<S> {
    pub session: S,
    pub models: Option<ModelStateFixture>,
    pub modes: Option<SessionModeState>,
}

pub struct TestConnectionConfig {
    pub mcp_servers: Vec<McpServer>,
    pub builtins: Vec<String>,
    pub goose_mode: GooseMode,
    pub cwd: Option<tempfile::TempDir>,
    pub data_root: PathBuf,
    pub provider_factory: Option<AcpProviderFactory>,
    pub read_text_file: Option<ReadTextFileHandler>,
    pub write_text_file: Option<WriteTextFileHandler>,
    pub terminal: Option<Arc<TerminalFixture>>,
    // The model the server-side provider starts with. Defaults to TEST_MODEL.
    pub current_model: String,
    pub disable_session_naming: bool,
}

impl Default for TestConnectionConfig {
    fn default() -> Self {
        Self {
            mcp_servers: Vec::new(),
            builtins: Vec::new(),
            goose_mode: GooseMode::default(),
            cwd: None,
            data_root: PathBuf::new(),
            provider_factory: None,
            read_text_file: None,
            write_text_file: None,
            terminal: None,
            current_model: TEST_MODEL.to_string(),
            disable_session_naming: true,
        }
    }
}

#[async_trait]
pub trait Connection: Sized {
    type Session: Session;

    fn expected_session_id() -> Arc<dyn ExpectedSessionId>;
    async fn new(config: TestConnectionConfig, openai: OpenAiFixture) -> Self;
    async fn new_session(&mut self) -> anyhow::Result<SessionData<Self::Session>>;
    async fn load_session(
        &mut self,
        session_id: &str,
        mcp_servers: Vec<McpServer>,
    ) -> anyhow::Result<SessionData<Self::Session>>;
    async fn list_sessions(&self) -> anyhow::Result<ListSessionsResponse>;
    async fn close_session(&self, session_id: &str) -> anyhow::Result<()>;
    async fn delete_session(&self, session_id: &str) -> anyhow::Result<()>;
    async fn set_mode(&self, session_id: &str, mode_id: &str) -> anyhow::Result<()>;
    async fn set_model(&self, session_id: &str, model_id: &str) -> anyhow::Result<()>;
    async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> anyhow::Result<()>;
    fn data_root(&self) -> std::path::PathBuf;
    fn reset_openai(&self);
    fn reset_permissions(&self);
}

#[async_trait]
pub trait Session: std::fmt::Debug {
    fn session_id(&self) -> &agent_client_protocol::schema::v1::SessionId;
    fn work_dir(&self) -> std::path::PathBuf;
    /// Drains and returns raw session updates collected by the fixture.
    fn session_updates(&self) -> Vec<SessionUpdate>;
    /// Drains and returns simplified notifications collected by the fixture.
    fn notifications(&self) -> Vec<Notification>;
    async fn prompt(
        &mut self,
        text: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<TestOutput>;
    async fn prompt_with_image(
        &mut self,
        text: &str,
        image_b64: &str,
        mime_type: &str,
        decision: PermissionDecision,
    ) -> anyhow::Result<TestOutput>;
}

#[allow(dead_code)]
pub fn run_test<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let _guard = ACP_TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if std::env::var_os("GOOSE_PATH_ROOT").is_none() {
        std::env::set_var("GOOSE_PATH_ROOT", ACP_CONFIG_ROOT.path());
    }
    register_builtin_extensions(goose_mcp::BUILTIN_EXTENSIONS.clone());

    let handle = std::thread::Builder::new()
        .name("acp-test".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(8 * 1024 * 1024)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(fut);
        })
        .unwrap();
    if let Err(err) = handle.join() {
        // Re-raise the original panic so the test shows the real failure message.
        std::panic::resume_unwind(err);
    }
}

pub async fn send_custom(
    cx: &agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let msg = agent_client_protocol::UntypedMessage::new(method, params).unwrap();
    cx.send_request(msg).block_task().await
}

pub mod provider;
pub mod server;
