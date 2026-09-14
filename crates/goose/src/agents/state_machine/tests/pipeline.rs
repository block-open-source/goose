use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Result;
use rmcp::model::ElicitationAction;
use tokio::sync::mpsc;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

use super::calculator_extension::CalculatorExtension;
use super::dummy_api::{DummyApi, ProviderFeatures};
use crate::action_required_manager::ElicitationOutcome;
use crate::agents::extension::ExtensionConfig;
use crate::agents::extension_manager::{ExtensionManager, ExtensionManagerCapabilities};
use crate::agents::mcp_client::McpClientTrait;
use crate::agents::prompt_manager::PromptManager;
use crate::agents::state_machine::{
    BangShellOperation, CompactionOperation, DoctorOperation, Emitter, EntryHookOperation,
    ExitOnErrorOperation, GooseEffect, GooseInferenceProvider, GooseInferenceRequestPreparer,
    InferenceRunner, MaxTurnsOperation, Operation, ProjectOperation, RecipeOperation,
    RetryOperation, SkillOperation, SlashCommandOperation, StateMachine, StatusOperation,
    SteerOperation, SteerQueue, Step, StopHookOperation, ToolApprovalOperation,
    ToolExecutionOperation, ToolPairCompactionOperation, UnknownToolOperation,
};
use crate::agents::AgentEvent;
use crate::config::permission::{PermissionLevel, PermissionManager};
use crate::config::GooseMode;
use crate::conversation::message::{ActionRequiredData, Message, MessageContent};
use crate::conversation::Conversation;
use crate::hooks::HookManager;
use crate::permission::permission_inspector::PermissionInspector;
use crate::permission::Permission;
use crate::providers::base::Provider;
use crate::security::security_inspector::SecurityInspector;
use crate::session::extension_data::EnabledExtensionsState;
use crate::session::{Session, SessionManager, SessionType};
use crate::tool_inspection::ToolInspectionManager;
use goose_providers::model::ModelConfig;

struct FeatureProvider {
    inner: Arc<dyn Provider>,
    features: ProviderFeatures,
}

#[async_trait::async_trait]
impl Provider for FeatureProvider {
    fn get_name(&self) -> &str {
        self.inner.get_name()
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[rmcp::model::Tool],
    ) -> Result<crate::providers::base::MessageStream, goose_providers::errors::ProviderError> {
        self.inner
            .stream(model_config, system, messages, tools)
            .await
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        self.inner.get_context_limit(model, override_limit).await
    }

    fn manages_own_context(&self) -> bool {
        self.features.manages_own_context
    }

    async fn fetch_model_info(
        &self,
        model_name: &str,
    ) -> Result<goose_providers::base::ModelInfo, goose_providers::errors::ProviderError> {
        let mut model_info = self.inner.fetch_model_info(model_name).await?;
        if let Some(resolved_model) = self.features.resolved_model {
            model_info.resolved_model = Some(resolved_model.to_string());
        }
        Ok(model_info)
    }
}

pub(super) const MAX_TURNS: u32 = 25;
pub(super) const COMPACTION_THRESHOLD: f64 = 0.8;

pub(super) struct TestPipeline {
    pub(super) session_manager: Arc<SessionManager>,
    api: Arc<DummyApi>,
    provider_features: ProviderFeatures,
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
    extension_manager: Arc<ExtensionManager>,
    goose_mode: TokioMutex<GooseMode>,
    prompt_manager: TokioMutex<PromptManager>,
    tool_inspection_manager: ToolInspectionManager,
    permission_manager: Arc<PermissionManager>,
    hook_manager: HookManager,
    stop_hook_block_cap: u32,
    goal: TokioMutex<Option<String>>,
    grind: TokioMutex<Option<String>>,
    calculator: Arc<CalculatorExtension>,
    pub(super) session_id: String,
    working_dir: std::path::PathBuf,
    steer_queue: SteerQueue,
    max_turns: u32,
    scheduler: Option<Arc<crate::scheduler::Scheduler>>,
    _temp_dir: Arc<tempfile::TempDir>,
}

impl TestPipeline {
    pub(super) fn machine(
        &self,
        cancel: CancellationToken,
    ) -> StateMachine<'_, Session, GooseEffect> {
        let provider = self.provider.clone();
        let tool_call_cutoff = crate::context_mgmt::compute_tool_call_cutoff(
            self.model_config.context_limit(),
            COMPACTION_THRESHOLD,
        );
        let mut operations: Vec<Arc<dyn Operation<Session, GooseEffect> + '_>> = vec![
            Arc::new(SteerOperation::new(
                self.steer_queue.clone(),
                self.hook_manager.clone(),
            )),
            Arc::new(MaxTurnsOperation::new(self.max_turns)),
            Arc::new(BangShellOperation::new()),
        ];
        if !self.provider_features.manages_own_context {
            operations.push(Arc::new(CompactionOperation::new(
                provider.clone(),
                self.model_config.clone(),
                self.model_config.context_limit(),
                COMPACTION_THRESHOLD,
            )));
        }
        let remaining_operations: Vec<Arc<dyn Operation<Session, GooseEffect> + '_>> = vec![
            Arc::new(ToolPairCompactionOperation::new(
                provider.clone(),
                self.model_config.clone(),
                tool_call_cutoff,
                !self.provider_features.manages_own_context,
            )),
            Arc::new(ToolApprovalOperation::new(
                &self.goose_mode,
                &self.tool_inspection_manager,
            )),
            Arc::new(DoctorOperation),
            Arc::new(ProjectOperation),
            Arc::new(SkillOperation::new(self.hook_manager.clone())),
            Arc::new(RecipeOperation::new(
                provider.clone(),
                self.hook_manager.clone(),
            )),
            Arc::new(ToolExecutionOperation::new(
                &self.goose_mode,
                self.extension_manager.clone(),
                self.hook_manager.clone(),
            )),
            Arc::new(UnknownToolOperation::new(self.hook_manager.clone())),
            Arc::new(RetryOperation::new(
                &self.goal,
                &self.grind,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
            )),
            Arc::new(StopHookOperation::new(
                self.hook_manager.clone(),
                self.stop_hook_block_cap,
            )),
            Arc::new(ExitOnErrorOperation),
        ];
        operations.extend(remaining_operations);
        let request_preparer = GooseInferenceRequestPreparer {
            #[cfg(feature = "code-mode")]
            extension_manager: self.extension_manager.clone(),
            goose_mode: &self.goose_mode,
            prompt_manager: &self.prompt_manager,
            tool_inspection_manager: &self.tool_inspection_manager,
            context_limit: self.model_config.context_limit(),
        };
        let status_operation = Arc::new(StatusOperation::new(
            provider.clone(),
            self.model_config.clone(),
        ));
        let inference_provider = Arc::new(GooseInferenceProvider::new(provider));
        let inference = Arc::new(
            InferenceRunner::new(inference_provider, self.model_config.clone())
                .with_request_preparer(Arc::new(request_preparer)),
        );
        let mut command_handlers = operations.clone();
        command_handlers.push(status_operation);
        let command_operation: Arc<dyn Operation<Session, GooseEffect> + '_> =
            Arc::new(SlashCommandOperation::new(command_handlers));
        let steps = std::iter::once(Arc::new(EntryHookOperation::new(self.hook_manager.clone()))
            as Arc<dyn Operation<Session, GooseEffect> + '_>)
        .chain(std::iter::once(command_operation))
        .chain(operations)
        .map(Step::Operation)
        .chain(std::iter::once(Step::Inference(inference)))
        .collect();

        StateMachine::new(steps, cancel)
    }

    pub(super) async fn with_goose_mode(self, mode: GooseMode) -> Self {
        *self.goose_mode.lock().await = mode;
        self.session_manager
            .update(&self.session_id)
            .goose_mode(mode)
            .apply()
            .await
            .unwrap();
        self
    }

    pub(super) fn context_limit(&self) -> usize {
        self.model_config.context_limit()
    }

    pub(super) async fn with_model(mut self, model: &str) -> Self {
        self.model_config = ModelConfig::new(model).with_canonical_limits("openai");
        self.session_manager
            .update(&self.session_id)
            .model_config(self.model_config.clone())
            .apply()
            .await
            .unwrap();
        self
    }

    pub(super) async fn with_model_config(mut self, model_config: ModelConfig) -> Self {
        self.model_config = model_config;
        self.session_manager
            .update(&self.session_id)
            .model_config(self.model_config.clone())
            .apply()
            .await
            .unwrap();
        self
    }

    pub(super) async fn with_provider_name(self, provider_name: &str) -> Result<Self> {
        self.session_manager
            .update(&self.session_id)
            .provider_name(provider_name)
            .apply()
            .await?;
        self.reconstruct().await
    }

    pub(super) fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    pub(super) fn working_dir(&self) -> &std::path::Path {
        &self.working_dir
    }

    pub(super) fn with_hook_manager(mut self, hook_manager: HookManager) -> Self {
        self.hook_manager = hook_manager;
        self
    }

    pub(super) fn with_stop_hook_block_cap(mut self, cap: u32) -> Self {
        self.stop_hook_block_cap = cap;
        self
    }

    pub(super) async fn set_total_tokens(&self, tokens: i32) {
        use goose_providers::conversation::token_usage::Usage;
        self.session_manager
            .update(&self.session_id)
            .usage(Usage::new(None, None, Some(tokens)))
            .apply()
            .await
            .unwrap();
    }

    pub(super) async fn set_recipe(&self, recipe: crate::recipe::Recipe) -> Result<()> {
        self.session_manager
            .update(&self.session_id)
            .recipe(Some(recipe.clone()))
            .apply()
            .await?;
        for extension in recipe.extensions.clone().unwrap_or_default() {
            self.extension_manager
                .add_extension(
                    extension,
                    Some(self.working_dir.clone()),
                    None,
                    Some(&self.session_id),
                )
                .await?;
        }
        Ok(())
    }

    pub(super) async fn set_schedule_id(&self, schedule_id: String) -> Result<()> {
        self.session_manager
            .update(&self.session_id)
            .schedule_id(Some(schedule_id))
            .apply()
            .await
    }

    pub(super) async fn get_goal(&self) -> Option<String> {
        self.goal.lock().await.clone()
    }

    pub(super) async fn set_grind(&self, grind: Option<String>) {
        *self.grind.lock().await = grind;
    }

    pub(super) async fn session(&self) -> Result<Session> {
        self.session_manager
            .get_session(&self.session_id, true)
            .await
    }

    pub(super) async fn steer(&self, message: Message) {
        self.steer_queue.lock().await.push_back(message);
    }

    pub(super) async fn has_pending_steers(&self) -> bool {
        !self.steer_queue.lock().await.is_empty()
    }

    pub(super) fn calculator_total(&self) -> i64 {
        self.calculator.total()
    }

    pub(super) async fn wait_for_calculator_result(&self) {
        self.calculator.wait_for_result().await;
    }

    pub(super) fn tool_contexts(&self) -> Vec<crate::agents::tool_execution::ToolCallContext> {
        self.calculator.contexts()
    }

    pub(super) async fn reconstruct(&self) -> Result<Self> {
        let session = self.session().await?;
        let goal = self.goal.lock().await.clone();
        let grind = self.grind.lock().await.clone();
        let pipeline = build_test_pipeline(
            self.session_manager.clone(),
            self.api.clone(),
            self.provider_features,
            self.scheduler.clone(),
            session,
            self._temp_dir.clone(),
        )
        .await?
        .with_hook_manager(self.hook_manager.clone())
        .with_stop_hook_block_cap(self.stop_hook_block_cap);
        *pipeline.goal.lock().await = goal;
        *pipeline.grind.lock().await = grind;
        Ok(pipeline)
    }

    pub(super) async fn new_session(&self, working_dir: std::path::PathBuf) -> Result<Self> {
        let source_session = self.session().await?;
        let session = self
            .session_manager
            .create_session(
                working_dir,
                "pipeline-test".to_string(),
                SessionType::Hidden,
                GooseMode::Auto,
            )
            .await?;
        self.session_manager
            .update(&session.id)
            .provider_name(
                source_session
                    .provider_name
                    .expect("source test session has a provider"),
            )
            .model_config(self.model_config.clone())
            .apply()
            .await?;
        build_test_pipeline(
            self.session_manager.clone(),
            self.api.clone(),
            self.provider_features,
            self.scheduler.clone(),
            self.session_manager.get_session(&session.id, true).await?,
            self._temp_dir.clone(),
        )
        .await
    }

    pub(super) fn synchronize_calculator(&self, calls: usize) {
        self.calculator.synchronize(calls);
    }

    pub(super) async fn run<const N: usize>(&self, user_messages: [&str; N]) -> Result<TestRun> {
        let mut events = Vec::new();

        for text in user_messages {
            self.session_manager
                .add_message(&self.session_id, &Message::user().with_text(text))
                .await?;

            events.extend(run_machine(self).await?);
        }

        Ok(TestRun::new(self.session().await?, events))
    }

    pub(super) async fn run_message(&self, message: Message) -> Result<TestRun> {
        self.session_manager
            .add_message(&self.session_id, &message)
            .await?;
        let events = run_machine(self).await?;
        Ok(TestRun::new(self.session().await?, events))
    }

    pub(super) async fn run_reconstructing_each_step(
        mut self,
        message: &str,
    ) -> Result<(Self, TestRun, usize)> {
        self.session_manager
            .add_message(&self.session_id, &Message::user().with_text(message))
            .await?;

        let cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel(1024);
        let emit = Emitter::new(tx, cancel.clone());
        let mut events = Vec::new();
        let mut applied_steps = 0;

        loop {
            let session = self.session().await?;
            let machine = self.machine(cancel.clone());
            let Some(mut result) = machine.step(&session, &emit).await? else {
                break;
            };
            machine
                .apply(self.session_manager.as_ref(), &session, &mut result, &emit)
                .await?;
            applied_steps += 1;
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }

            let yield_to_client = result.yield_to_client;
            drop(machine);
            self = self.reconstruct().await?;
            if yield_to_client {
                break;
            }
        }

        drop(emit);
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        let result = TestRun::new(self.session().await?, events);
        Ok((self, result, applied_steps))
    }

    pub(super) async fn seed<const N: usize>(&self, messages: [Message; N]) -> Result<()> {
        for message in messages {
            self.session_manager
                .add_message(&self.session_id, &message)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn resume(&self) -> Result<TestRun> {
        let events = run_machine(self).await?;
        Ok(TestRun::new(self.session().await?, events))
    }

    pub(super) async fn confirm(&self, id: &str, permission: Permission) -> Result<()> {
        self.session_manager
            .add_message(
                &self.session_id,
                &Message::user()
                    .with_content(MessageContent::action_required_tool_confirmation_response(
                        id, permission,
                    ))
                    .with_visibility(false, false),
            )
            .await
    }

    pub(super) fn set_permission(&self, tool: &str, level: PermissionLevel) {
        self.permission_manager.update_user_permission(tool, level);
    }

    pub(super) async fn remove_extension(&self, name: &str) -> Result<()> {
        self.extension_manager
            .remove_extension(name)
            .await
            .map_err(anyhow::Error::from)
    }

    pub(super) async fn add_extension(&self, name: &str) -> Result<()> {
        self.extension_manager
            .add_extension(
                ExtensionConfig::Platform {
                    name: name.to_string(),
                    description: name.to_string(),
                    display_name: None,
                    bundled: None,
                    available_tools: vec![],
                },
                Some(self.working_dir.clone()),
                None,
                Some(&self.session_id),
            )
            .await
            .map_err(anyhow::Error::from)
    }

    pub(super) async fn resume_cancelled(&self) -> Result<TestRun> {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let machine = self.machine(cancel.clone());
        let (tx, mut rx) = mpsc::channel(1024);
        let emit = Emitter::new(tx, cancel);
        let session = machine
            .run(self.session_manager.as_ref(), &self.session_id, &emit)
            .await?;
        drop(emit);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        Ok(TestRun::new(session, events))
    }

    pub(super) async fn run_with_cancel(
        &self,
        message: &str,
        cancel: CancellationToken,
    ) -> Result<TestRun> {
        self.session_manager
            .add_message(&self.session_id, &Message::user().with_text(message))
            .await?;
        let machine = self.machine(cancel.clone());
        let (tx, mut rx) = mpsc::channel(1024);
        let emit = Emitter::new(tx, cancel);
        let session = machine
            .run(self.session_manager.as_ref(), &self.session_id, &emit)
            .await?;
        drop(emit);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        Ok(TestRun::new(session, events))
    }

    pub(super) async fn run_with_elicitation(
        &self,
        message: &str,
        action: ElicitationAction,
        user_data: serde_json::Value,
    ) -> Result<TestRun> {
        self.session_manager
            .add_message(&self.session_id, &Message::user().with_text(message))
            .await?;
        let cancel = CancellationToken::new();
        let machine = self.machine(cancel.clone());
        let (tx, mut rx) = mpsc::channel(1024);
        let emit = Emitter::new(tx, cancel);
        let mut events = Vec::new();
        let mut answered = false;

        loop {
            let session = self.session().await?;
            let step = machine.step(&session, &emit);
            tokio::pin!(step);
            let mut result = loop {
                tokio::select! {
                    result = &mut step => break result?,
                    Some(event) = rx.recv() => {
                        if let AgentEvent::Message(message) = &event {
                            let elicitation_id = message.content.iter().find_map(|content| {
                                match content {
                                    MessageContent::ActionRequired(action) => match &action.data {
                                        ActionRequiredData::Elicitation { id, .. } => Some(id.clone()),
                                        _ => None,
                                    },
                                    _ => None,
                                }
                            });
                            if let Some(id) = elicitation_id {
                                let outcome = match &action {
                                    ElicitationAction::Accept => {
                                        ElicitationOutcome::Accept(user_data.clone())
                                    }
                                    ElicitationAction::Decline => ElicitationOutcome::Decline,
                                    ElicitationAction::Cancel => ElicitationOutcome::Cancel,
                                    _ => ElicitationOutcome::Cancel,
                                };
                                let response = Message::user()
                                    .with_generated_id()
                                    .with_content(
                                        MessageContent::action_required_elicitation_response(
                                            id.clone(),
                                            user_data.clone(),
                                            action.clone(),
                                        ),
                                    )
                                    .agent_only();
                                crate::elicitation::complete_elicitation_with_message(
                                    &self.session_manager,
                                    &self.session_id,
                                    &id,
                                    outcome,
                                    &response,
                                )
                                .await?;
                                answered = true;
                            }
                        }
                        events.push(event);
                    }
                }
            };
            let Some(ref mut result) = result else {
                break;
            };
            machine
                .apply(self.session_manager.as_ref(), &session, result, &emit)
                .await?;
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            if result.yield_to_client {
                break;
            }
        }
        assert!(answered, "tool did not request elicitation");
        drop(emit);
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        Ok(TestRun::new(self.session().await?, events))
    }

    pub(super) async fn set_system_prompt_override(&self, prompt: impl Into<String>) {
        self.prompt_manager
            .lock()
            .await
            .set_system_prompt_override(prompt.into());
    }

    pub(super) async fn clear_system_prompt_override(&self) {
        self.prompt_manager
            .lock()
            .await
            .clear_system_prompt_override();
    }
}

pub(super) async fn test_pipeline() -> Result<(TestPipeline, Arc<DummyApi>)> {
    test_pipeline_with(ProviderFeatures::default()).await
}

pub(super) async fn test_pipeline_with_scheduler() -> Result<(
    TestPipeline,
    Arc<DummyApi>,
    Arc<crate::scheduler::Scheduler>,
)> {
    let (pipeline, api, scheduler) =
        test_pipeline_with_components(ProviderFeatures::default(), true).await?;
    Ok((pipeline, api, scheduler.expect("scheduler was requested")))
}

pub(super) async fn test_pipeline_with(
    features: ProviderFeatures,
) -> Result<(TestPipeline, Arc<DummyApi>)> {
    let (pipeline, api, _) = test_pipeline_with_components(features, false).await?;
    Ok((pipeline, api))
}

async fn test_pipeline_with_components(
    features: ProviderFeatures,
    with_scheduler: bool,
) -> Result<(
    TestPipeline,
    Arc<DummyApi>,
    Option<Arc<crate::scheduler::Scheduler>>,
)> {
    let api = Arc::new(DummyApi::start(features).await);
    let temp_dir = Arc::new(tempfile::tempdir()?);
    let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
    let scheduler = if with_scheduler {
        Some(
            crate::scheduler::Scheduler::new(
                temp_dir.path().join("schedule.json"),
                session_manager.clone(),
            )
            .await?,
        )
    } else {
        None
    };
    let session = session_manager
        .create_session(
            temp_dir.path().to_path_buf(),
            "pipeline-test".to_string(),
            if with_scheduler {
                SessionType::Scheduled
            } else {
                SessionType::Hidden
            },
            GooseMode::Auto,
        )
        .await?;
    let model_config = ModelConfig::new(goose_providers::openai::OPEN_AI_DEFAULT_MODEL)
        .with_canonical_limits("openai");
    session_manager
        .update(&session.id)
        .provider_name("openai")
        .model_config(model_config)
        .apply()
        .await?;
    let session = session_manager.get_session(&session.id, true).await?;
    let pipeline = build_test_pipeline(
        session_manager,
        api.clone(),
        features,
        scheduler.clone(),
        session,
        temp_dir,
    )
    .await?;

    Ok((pipeline, api, scheduler))
}

async fn build_test_pipeline(
    session_manager: Arc<SessionManager>,
    api: Arc<DummyApi>,
    provider_features: ProviderFeatures,
    scheduler: Option<Arc<crate::scheduler::Scheduler>>,
    session: Session,
    temp_dir: Arc<tempfile::TempDir>,
) -> Result<TestPipeline> {
    let provider_name = session
        .provider_name
        .as_deref()
        .expect("test session has a provider");
    let api_client = goose_providers::api_client::ApiClient::new_with_tls(
        api.uri(),
        goose_providers::api_client::AuthMethod::NoAuth,
        None,
    )?;
    let provider: Arc<dyn Provider> = Arc::new(
        goose_providers::openai::OpenAiProviderBuilder::new(api_client)
            .name(provider_name)
            .preserve_thinking_context(provider_features.preserves_thinking)
            .build(),
    );
    let provider: Arc<dyn Provider> =
        if provider_features.resolved_model.is_some() || provider_features.manages_own_context {
            Arc::new(FeatureProvider {
                inner: provider,
                features: provider_features,
            })
        } else {
            provider
        };
    let shared_provider = Arc::new(TokioMutex::new(Some(provider.clone())));
    let extension_manager = Arc::new(ExtensionManager::new(
        shared_provider.clone(),
        session_manager.clone(),
        scheduler
            .clone()
            .map(|scheduler| scheduler as Arc<dyn crate::scheduler_trait::SchedulerTrait>),
        "pipeline-test".to_string(),
        ExtensionManagerCapabilities {
            mcpui: false,
            host_info: None,
            elicitation_handler: None,
            protocol_version: None,
        },
        false,
    ));
    let permission_manager = Arc::new(PermissionManager::new(temp_dir.path().join("permissions")));
    let mut tool_inspection_manager = ToolInspectionManager::new();
    tool_inspection_manager.add_inspector(Box::new(SecurityInspector::enabled()));
    tool_inspection_manager.add_inspector(Box::new(PermissionInspector::new(
        permission_manager.clone(),
        shared_provider,
        session_manager.clone(),
    )));
    let model_config = session
        .model_config
        .clone()
        .expect("test session has a model config");
    let calculator = Arc::new(CalculatorExtension::new(session_manager.action_required()));
    let pipeline = TestPipeline {
        session_manager,
        api,
        provider_features,
        provider: provider.clone(),
        model_config,
        extension_manager,
        goose_mode: TokioMutex::new(session.goose_mode),
        prompt_manager: TokioMutex::new(PromptManager::new()),
        tool_inspection_manager,
        permission_manager,
        hook_manager: HookManager::default(),
        stop_hook_block_cap: 3,
        goal: TokioMutex::new(None),
        grind: TokioMutex::new(None),
        calculator: calculator.clone(),
        session_id: session.id.clone(),
        working_dir: session.working_dir.clone(),
        steer_queue: Arc::new(tokio::sync::Mutex::new(VecDeque::new())),
        max_turns: MAX_TURNS,
        scheduler,
        _temp_dir: temp_dir,
    };
    let extension_manager = pipeline.extension_manager.clone();
    let session_id = pipeline.session_id.clone();
    let mut extensions = EnabledExtensionsState::from_extension_data(&session.extension_data)
        .map(|state| state.extensions)
        .unwrap_or_else(default_extensions);
    for recipe_extension in session
        .recipe
        .as_ref()
        .and_then(|recipe| recipe.extensions.as_ref())
        .into_iter()
        .flatten()
    {
        if !extensions
            .iter()
            .any(|extension| extension.name() == recipe_extension.name())
        {
            extensions.push(recipe_extension.clone());
        }
    }
    if !extensions
        .iter()
        .any(|extension| extension.name() == "calculator")
    {
        extensions.push(platform_extension("calculator", "Stateful test calculator"));
    }
    for extension in extensions {
        if extension.name() == "calculator" {
            extension_manager
                .add_client(
                    "calculator".to_string(),
                    extension,
                    calculator.clone(),
                    calculator.get_info().cloned(),
                )
                .await;
        } else {
            extension_manager
                .add_extension(
                    extension,
                    Some(session.working_dir.clone()),
                    None,
                    Some(&session_id),
                )
                .await?;
        }
    }

    Ok(pipeline)
}

fn default_extensions() -> Vec<ExtensionConfig> {
    [
        ("calculator", "Stateful test calculator"),
        ("extensionmanager", "Extension Manager"),
        ("todo", "Todo"),
        (
            crate::agents::platform_extensions::scheduler::EXTENSION_NAME,
            "Scheduler",
        ),
    ]
    .into_iter()
    .map(|(name, description)| platform_extension(name, description))
    .collect()
}

fn platform_extension(name: &str, description: &str) -> ExtensionConfig {
    ExtensionConfig::Platform {
        name: name.to_string(),
        description: description.to_string(),
        display_name: None,
        bundled: None,
        available_tools: vec![],
    }
}

pub(super) struct TestRun {
    pub session: Session,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum MessageKind {
    Agent,
    Confirmation,
    Error,
    Thinking,
    ToolCall,
    ToolResponse,
    User,
}

impl TestRun {
    fn new(session: Session, events: Vec<AgentEvent>) -> Self {
        let conversation = session
            .conversation
            .as_ref()
            .expect("session has a conversation");
        for message in conversation.messages() {
            assert!(
                message.id.is_some(),
                "persisted message has no id: {message:#?}"
            );
        }
        for event in &events {
            if let AgentEvent::Message(message) = event {
                assert!(
                    message.id.is_some(),
                    "emitted message has no id: {message:#?}"
                );
            }
        }
        Self { session, events }
    }

    pub(super) fn conversation(&self) -> &Conversation {
        self.session
            .conversation
            .as_ref()
            .expect("session has a conversation")
    }

    pub(super) fn history_replacements(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, AgentEvent::HistoryReplaced(_)))
            .count()
    }

    pub(super) fn assert_emitted(&self, contains: &str) {
        let emitted = self
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Message(message) => Some(
                    message
                        .content
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            emitted.iter().any(|message| message.contains(contains)),
            "no emitted message contains {contains:?}:\n{}",
            emitted.join("\n")
        );
    }

    pub(super) fn assert_emitted_message_matches_persisted(&self, contains: &str) {
        let emitted = self
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::Message(message) if message.as_concat_text().contains(contains) => {
                    Some(message)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no emitted message contains {contains:?}"));
        let persisted = self
            .conversation()
            .messages()
            .iter()
            .find(|message| message.as_concat_text().contains(contains))
            .unwrap_or_else(|| panic!("no persisted message contains {contains:?}"));
        assert_eq!(emitted.id, persisted.id);
    }

    pub(super) fn assert_message(&self, index: isize, kind: MessageKind, contains: &str) {
        let messages = self
            .conversation()
            .messages()
            .iter()
            .filter(|message| !message.is_turn_context())
            .flat_map(|message| {
                message
                    .content
                    .iter()
                    .map(move |content| (message, content))
            })
            .collect::<Vec<_>>();
        let resolved = if index < 0 {
            messages.len() as isize + index
        } else {
            index
        };
        assert!(
            resolved >= 0 && resolved < messages.len() as isize,
            "message index {index} is out of bounds for {} content blocks",
            messages.len()
        );
        let (message, content) = messages[resolved as usize];
        let content = match (kind, content) {
            (MessageKind::Agent, MessageContent::Text(text))
                if message.role == rmcp::model::Role::Assistant =>
            {
                text.text.clone()
            }
            (MessageKind::User, MessageContent::Text(text))
                if message.role == rmcp::model::Role::User =>
            {
                text.text.clone()
            }
            (MessageKind::Thinking, MessageContent::Thinking(thinking)) => {
                thinking.thinking.clone()
            }
            (MessageKind::ToolCall, MessageContent::ToolRequest(request)) => {
                match &request.tool_call {
                    Ok(call) => {
                        let arguments = call
                            .arguments
                            .clone()
                            .map(serde_json::Value::Object)
                            .unwrap_or_default();
                        format!("{}({arguments})", call.name)
                    }
                    Err(error) => error.message.to_string(),
                }
            }
            (MessageKind::ToolResponse, MessageContent::ToolResponse(response)) => {
                match response.tool_result.as_ref() {
                    Ok(result) => result
                        .content
                        .iter()
                        .filter_map(|content| content.as_text().map(|text| text.text.clone()))
                        .collect(),
                    Err(error) => error.message.to_string(),
                }
            }
            (MessageKind::Confirmation, MessageContent::ActionRequired(action)) => {
                match &action.data {
                    ActionRequiredData::ToolConfirmation {
                        id,
                        tool_name,
                        prompt,
                        ..
                    } => format!("{id} {tool_name} {}", prompt.as_deref().unwrap_or_default()),
                    _ => panic!("message {index} is not a tool confirmation: {content:?}"),
                }
            }
            (MessageKind::Error, MessageContent::Error(error)) => error.message.clone(),
            _ => panic!("message {index} is not {kind:?}: {content:?}"),
        };
        assert!(
            content.contains(contains),
            "message {index} does not contain {contains:?}: {content:?}"
        );
    }
}

pub(super) async fn run_machine(pipeline: &TestPipeline) -> Result<Vec<AgentEvent>> {
    let cancel = CancellationToken::new();
    let machine = pipeline.machine(cancel.clone());
    let (tx, mut rx) = mpsc::channel(1024);
    let emit = Emitter::new(tx, cancel);
    let mut events = Vec::new();
    loop {
        let session = pipeline.session().await?;
        let Some(mut result) = machine.step(&session, &emit).await? else {
            break;
        };
        machine
            .apply(
                pipeline.session_manager.as_ref(),
                &session,
                &mut result,
                &emit,
            )
            .await?;
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        if result.yield_to_client {
            break;
        }
    }
    drop(emit);
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    Ok(events)
}
