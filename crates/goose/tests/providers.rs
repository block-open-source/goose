use anyhow::Result;
use dotenvy::dotenv;
use futures::StreamExt;
use goose::acp::ACP_CURRENT_MODEL;
use goose::agents::{Agent, AgentConfig, AgentEvent, GoosePlatform, PromptManager, SessionConfig};
use goose::config::{ExtensionConfig, GooseMode, PermissionManager};
use goose::conversation::message::{ActionRequiredData, Message, MessageContent};
use goose::permission::Permission;
use goose::providers::anthropic::ANTHROPIC_DEFAULT_MODEL;
use goose::providers::azure::AZURE_DEFAULT_MODEL;
use goose::providers::base::Provider;
#[cfg(feature = "aws-providers")]
use goose::providers::bedrock::BEDROCK_DEFAULT_MODEL;
use goose::providers::claude_code::CLAUDE_CODE_DEFAULT_MODEL;
use goose::providers::codex::CODEX_DEFAULT_MODEL;
use goose::providers::create_with_named_model;
use goose::providers::google::GOOGLE_DEFAULT_MODEL;
use goose::providers::litellm::LITELLM_DEFAULT_MODEL;
use goose::providers::openai::OPEN_AI_DEFAULT_MODEL;
#[cfg(feature = "aws-providers")]
use goose::providers::sagemaker_tgi::SAGEMAKER_TGI_DEFAULT_MODEL;
use goose::providers::snowflake::SNOWFLAKE_DEFAULT_MODEL;
use goose::providers::xai::XAI_DEFAULT_MODEL;
use goose::session::{SessionManager, SessionType};
use goose_providers::databricks::DATABRICKS_DEFAULT_MODEL;
use goose_providers::errors::ProviderError;
use goose_providers::thinking::ThinkingEffortSupport;
use goose_test_support::{
    EnforceSessionId, ExpectedSessionId, IgnoreSessionId, McpFixture, FAKE_CODE,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
enum TestStatus {
    Passed,
    Skipped,
    Failed,
}

impl std::fmt::Display for TestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestStatus::Passed => write!(f, "✅"),
            TestStatus::Skipped => write!(f, "⏭️"),
            TestStatus::Failed => write!(f, "❌"),
        }
    }
}

struct TestReport {
    results: Mutex<HashMap<String, TestStatus>>,
}

impl TestReport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(HashMap::new()),
        })
    }

    fn record_status(&self, provider: &str, status: TestStatus) {
        let mut results = self.results.lock().unwrap();
        results.insert(provider.to_string(), status);
    }

    fn record_pass(&self, provider: &str) {
        self.record_status(provider, TestStatus::Passed);
    }

    fn record_skip(&self, provider: &str) {
        self.record_status(provider, TestStatus::Skipped);
    }

    fn record_fail(&self, provider: &str) {
        self.record_status(provider, TestStatus::Failed);
    }

    fn print_summary(&self) {
        println!("\n============== Providers ==============");
        let results = self.results.lock().unwrap();
        let mut providers: Vec<_> = results.iter().collect();
        providers.sort_by(|a, b| a.0.cmp(b.0));

        for (provider, status) in providers {
            println!("{} {}", status, provider);
        }
        println!("=======================================\n");
    }
}

static TEST_REPORT: std::sync::LazyLock<Arc<TestReport>> =
    std::sync::LazyLock::new(TestReport::new);
static ENV_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

struct ProviderFixture {
    name: String,
    image_model: Option<String>,
    model_switch_name: Option<String>,
    expect_context_length_exceeded: bool,
    context_length_exceeded: usize,
    provider: Arc<dyn Provider>,
    model_config: goose_providers::model::ModelConfig,
    agent: Agent,
    session_id: String,
    _mcp: McpFixture,
    _guard: env_lock::EnvGuard<'static>,
    _temp_dir: tempfile::TempDir,
}

struct ProviderTestConfig {
    name: &'static str,
    model_name: &'static str,
    required_vars: &'static [&'static str],
    model_switch_name: Option<&'static str>,
    image_model: Option<&'static str>,
    clear_env: &'static [&'static str],
    skip: bool,
    expected_session_id: fn() -> Arc<dyn ExpectedSessionId>,
    test_permissions: bool,
    test_smart_approve: bool,
    test_mode_update: bool,
    test_mcp_tools: bool,
    test_context_length_exceeded: bool,
    test_thinking_effort: bool,
    expect_context_length_exceeded: bool,
    context_length_exceeded: usize,
}

impl ProviderTestConfig {
    fn with_llm_provider(
        name: &'static str,
        model_name: &'static str,
        required_vars: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            model_name,
            required_vars,
            model_switch_name: None,
            image_model: None,
            clear_env: &[],
            skip: false,
            expected_session_id: || Arc::new(EnforceSessionId::default()),
            test_permissions: true,
            test_smart_approve: true,
            test_mode_update: true,
            test_mcp_tools: true,
            test_context_length_exceeded: true,
            test_thinking_effort: false,
            expect_context_length_exceeded: true,
            context_length_exceeded: 600_000,
        }
    }

    fn model_switch_name(mut self, name: &'static str) -> Self {
        self.model_switch_name = Some(name);
        self
    }

    fn image_model(mut self, name: &'static str) -> Self {
        self.image_model = Some(name);
        self
    }

    fn test_permissions(mut self, v: bool) -> Self {
        self.test_permissions = v;
        self
    }

    fn test_smart_approve(mut self, v: bool) -> Self {
        self.test_smart_approve = v;
        self
    }

    fn test_mcp_tools(mut self, v: bool) -> Self {
        self.test_mcp_tools = v;
        self
    }

    fn test_thinking_effort(mut self, v: bool) -> Self {
        self.test_thinking_effort = v;
        self
    }

    fn expect_context_length_exceeded(mut self, v: bool) -> Self {
        self.expect_context_length_exceeded = v;
        self
    }

    fn context_length_exceeded(mut self, token_count: usize) -> Self {
        self.context_length_exceeded = token_count;
        self
    }

    #[allow(dead_code)] // only used by tests that are behind non-default feature flags
    fn clear_env(mut self, vars: &'static [&'static str]) -> Self {
        self.clear_env = vars;
        self
    }

    fn with_agentic_provider(name: &'static str, model_name: &'static str, binary: &str) -> Self {
        let skip = which::which(binary).is_err();
        Self {
            skip,
            expected_session_id: || Arc::new(IgnoreSessionId),
            test_smart_approve: false,
            test_mode_update: false,
            test_context_length_exceeded: false,
            ..Self::with_llm_provider(name, model_name, &[])
        }
    }

    async fn run(self) -> Result<()> {
        test_provider(self).await
    }
}

impl ProviderFixture {
    async fn setup(config: &ProviderTestConfig, mode: GooseMode) -> Result<Self> {
        let mut env_vars: Vec<(&'static str, Option<&str>)> =
            vec![("GOOSE_MODE", Some(<&str>::from(mode)))];
        for &var in config.clear_env {
            env_vars.push((var, None));
        }
        let guard = env_lock::lock_env(env_vars);

        let expected_session_id = (config.expected_session_id)();
        let mcp = McpFixture::new().await;

        let mcp_extension =
            ExtensionConfig::streamable_http("mcp-fixture", &mcp.url, "MCP fixture", 30_u64);
        let developer_extension = ExtensionConfig::Builtin {
            name: "developer".to_string(),
            description: String::new(),
            display_name: Some("Developer".to_string()),
            timeout: None,
            bundled: None,
            available_tools: vec![],
        };

        let provider = create_with_named_model(
            &config.name.to_lowercase(),
            vec![mcp_extension.clone(), developer_extension.clone()],
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
        let model_config = goose::model_config::model_config_from_user_config(
            &config.name.to_lowercase(),
            config.model_name,
        )?;

        let temp_dir = tempfile::tempdir()?;
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let permission_manager = Arc::new(PermissionManager::new(temp_dir.path().to_path_buf()));

        let agent = Agent::with_config(AgentConfig::new(
            session_manager.clone(),
            permission_manager,
            None,
            mode,
            true,
            GoosePlatform::GooseCli,
        ));
        let session = session_manager
            .create_session(
                std::env::current_dir()?,
                "provider_test".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await?;
        let session_id = session.id;
        expected_session_id.set(&session_id);
        agent
            .update_provider(provider.clone(), model_config.clone(), &session_id)
            .await?;
        agent
            .add_extension(mcp_extension, &session_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        agent
            .add_extension(developer_extension, &session_id)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(Self {
            name: config.name.to_string(),
            image_model: config.image_model.map(String::from),
            model_switch_name: config.model_switch_name.map(String::from),
            expect_context_length_exceeded: config.expect_context_length_exceeded,
            context_length_exceeded: config.context_length_exceeded,
            provider,
            model_config,
            agent,
            session_id: session_id.to_string(),
            _mcp: mcp,
            _guard: guard,
            _temp_dir: temp_dir,
        })
    }

    async fn tool_roundtrip(
        &self,
        prompt: &str,
        model_config: Option<goose_providers::model::ModelConfig>,
    ) -> Result<Message> {
        let tools = self
            .agent
            .extension_manager
            .get_prefixed_tools(&self.session_id, None)
            .await
            .unwrap();

        let info = self
            .agent
            .extension_manager
            .get_extensions_info(std::path::Path::new("."))
            .await;
        let system = PromptManager::new()
            .builder()
            .with_extensions(info.into_iter())
            .build();

        let message = Message::user().with_text(prompt);
        let model_config = model_config.unwrap_or_else(|| self.model_config.clone());
        let (response1, _) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider.complete(
                &model_config,
                &system,
                std::slice::from_ref(&message),
                &tools,
            ),
        )
        .await?;

        // Agentic CLI providers (claude-code, codex) call tools internally and
        // return the final text result directly — no tool_request in the response.
        let tool_req = response1
            .content
            .iter()
            .filter_map(|c| c.as_tool_request())
            .next_back();

        let tool_req = match tool_req {
            Some(req) => req,
            None => return Ok(response1),
        };

        // Provider already executed an externally-dispatched tool — don't redispatch.
        if tool_req.was_executed_externally() {
            return Ok(response1);
        }

        let params = tool_req.tool_call.as_ref().unwrap().clone();
        let ctx = goose::agents::ToolCallContext::new(
            self.session_id.to_string(),
            None,
            Some("test-id".to_string()),
        );
        let result = self
            .agent
            .extension_manager
            .dispatch_tool_call(&ctx, params, CancellationToken::new())
            .await
            .unwrap()
            .result
            .await
            .unwrap();
        let tool_response = Message::user().with_tool_response(&tool_req.id, Ok(result));

        let (response2, _) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider.complete(
                &model_config,
                &system,
                &[message, response1, tool_response],
                &tools,
            ),
        )
        .await?;
        Ok(response2)
    }

    async fn test_basic_response(&self) -> Result<()> {
        let message = Message::user().with_text("Just say hello!");
        let model_config = self.model_config.clone();

        let (response, _) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider.complete(
                &model_config,
                "You are a helpful assistant.",
                &[message],
                &[],
            ),
        )
        .await?;

        assert!(!response.content.is_empty());
        assert!(response
            .content
            .iter()
            .any(|c| matches!(c, MessageContent::Text(_))));

        println!(
            "=== {}::basic_response === {}",
            self.name,
            response.as_concat_text()
        );
        Ok(())
    }

    async fn test_tool_usage(&self) -> Result<()> {
        let response = self
            .tool_roundtrip("Use the get_code tool and output only its result.", None)
            .await?;
        let text = response.as_concat_text();
        assert!(text.contains(FAKE_CODE), "{text}");
        println!("=== {}::tool_usage === {}", self.name, text);
        Ok(())
    }

    async fn test_context_length_exceeded_error(&self) -> Result<()> {
        // "hello " ≈ 2 tokens across common tokenizers
        let large_message_content = "hello ".repeat(self.context_length_exceeded / 2);
        let messages = vec![Message::user().with_text(&large_message_content)];
        let model_config = self.model_config.clone();

        let result = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider.complete(
                &model_config,
                "You are a helpful assistant.",
                &messages,
                &[],
            ),
        )
        .await;

        println!("=== {}::context_length_exceeded_error ===", self.name);
        dbg!(&result);
        println!("===================");

        if self.expect_context_length_exceeded {
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                ProviderError::ContextLengthExceeded(_)
            ));
        } else {
            assert!(result.is_ok());
        }

        Ok(())
    }

    async fn test_image_content_support(&self) -> Result<()> {
        let image_config = self.image_model.as_ref().map(|model| {
            goose_providers::model::ModelConfig::new(model).with_canonical_limits(&self.name)
        });
        let response = self
            .tool_roundtrip(
                "Use the get_image tool and describe what you see in its result.",
                image_config,
            )
            .await?;
        let text = response.as_concat_text().to_lowercase();
        assert!(
            text.contains("hello goose") || text.contains("test image"),
            "{text}"
        );
        println!("=== {}::image_content === {}", self.name, text);
        Ok(())
    }

    async fn test_model_switch(&self) -> Result<()> {
        let default = &self.model_config.model_name;
        let alt = self.model_switch_name.as_deref().unwrap();
        let alt_config =
            goose_providers::model::ModelConfig::new(alt).with_canonical_limits(&self.name);

        let message = Message::user().with_text("Just say hello!");
        let (response, _) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider
                .complete(&alt_config, "You are a helpful assistant.", &[message], &[]),
        )
        .await?;

        assert!(response
            .content
            .iter()
            .any(|c| matches!(c, MessageContent::Text(_))));
        println!(
            "=== {}::model_switch ({} -> {}) === {}",
            self.name,
            default,
            alt,
            response.as_concat_text()
        );
        Ok(())
    }

    async fn test_thinking_effort(&self) -> Result<()> {
        let ThinkingEffortSupport::Options(capability) = self.provider.thinking_effort_support()
        else {
            anyhow::bail!("{} should mirror the agent's effort option", self.name);
        };
        assert!(!capability.values.is_empty());
        let target = capability.values.last().unwrap().value.clone();
        println!(
            "=== {}::thinking_effort ({} -> {}) === {:?}",
            self.name,
            capability.current.as_deref().unwrap_or("unset"),
            target,
            capability.values
        );

        assert!(
            self.provider
                .set_thinking_effort(&self.session_id, &target)
                .await?
        );

        let effort_config = self
            .model_config
            .clone()
            .with_merged_request_params(HashMap::from([(
                "thinking_effort".to_string(),
                serde_json::json!(target),
            )]));
        let message = Message::user().with_text("Just say hello!");
        let (response, _) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            self.provider.complete(
                &effort_config,
                "You are a helpful assistant.",
                &[message],
                &[],
            ),
        )
        .await?;

        assert!(response
            .content
            .iter()
            .any(|c| matches!(c, MessageContent::Text(_))));
        Ok(())
    }

    async fn test_model_listing(&self) -> Result<()> {
        let models = self.provider.fetch_supported_models().await?;

        println!("=== {}::model_listing ===", self.name);
        dbg!(&models);
        println!("===================");

        assert!(!models.is_empty());
        let resolved = &self.model_config.model_name;
        if resolved != ACP_CURRENT_MODEL {
            assert!(models
                .iter()
                .any(|m| m == resolved || m.contains(resolved) || resolved.contains(m)));
        }
        if let Some(alt) = &self.model_switch_name {
            assert!(models
                .iter()
                .any(|m| m == alt || m.contains(alt.as_str()) || alt.contains(m.as_str())));
        }
        Ok(())
    }

    async fn run_permission_test(
        &self,
        permission: Permission,
        expect_action_required: bool,
        message: &str,
        label: &str,
    ) -> Result<()> {
        let message = Message::user().with_text(message);
        let session_config = SessionConfig {
            id: self.session_id.clone(),
            schedule_id: None,
            max_turns: Some(5),
            retry_config: None,
        };

        let mut stream = self
            .agent
            .reply(
                message,
                session_config,
                goose::agents::state_machine::enabled(),
                None,
            )
            .await?;
        let mut saw_action_required = false;

        while let Some(event) = stream.next().await {
            let event = event?;
            if let AgentEvent::Message(ref msg) = event {
                for content in &msg.content {
                    if let MessageContent::ActionRequired(ar) = content {
                        if let ActionRequiredData::ToolConfirmation { ref id, .. } = ar.data {
                            saw_action_required = true;
                            self.agent
                                .submit_tool_confirmation(&self.session_id, id, permission.clone())
                                .await?;
                        }
                    }
                }
            }
        }

        assert_eq!(saw_action_required, expect_action_required);
        println!("=== {}::{} ===", self.name, label);
        Ok(())
    }

    async fn test_permission_allow(&self) -> Result<()> {
        let test_file = tempfile::NamedTempFile::new()?;
        self.run_permission_test(
            Permission::AllowOnce,
            true,
            &format!("Write the word 'hello' to {}", test_file.path().display()),
            "permission_allow",
        )
        .await
    }

    async fn test_permission_deny(&self) -> Result<()> {
        let test_file = tempfile::NamedTempFile::new()?;
        self.run_permission_test(
            Permission::DenyOnce,
            true,
            &format!("Write the word 'hello' to {}", test_file.path().display()),
            "permission_deny",
        )
        .await
    }

    async fn test_smart_approve_llm_detect(&self) -> Result<()> {
        self.run_permission_test(
            Permission::AllowOnce,
            false,
            "Use the get_image tool and describe what you see in its result.",
            "smart_approve_llm_detect",
        )
        .await
    }

    async fn test_smart_approve_readonly(&self) -> Result<()> {
        self.run_permission_test(
            Permission::AllowOnce,
            false,
            "Use the get_code tool and output only its result.",
            "smart_approve_readonly",
        )
        .await
    }

    async fn test_mode_update(&self) -> Result<()> {
        // Start in Auto mode (fixture default), tools auto-approved.
        // Switch to Approve mode dynamically via agent.
        self.agent
            .update_goose_mode(GooseMode::Approve, &self.session_id)
            .await?;
        // Verify tool call now requires permission (ActionRequired).
        // Cancel prevents the task from completing → tool fails.
        self.run_permission_test(
            Permission::Cancel,
            true,
            "Use the get_code tool and output only its result.",
            "mode_update",
        )
        .await
    }
}

fn load_env() {
    if let Ok(path) = dotenv() {
        println!("Loaded environment from {:?}", path);
    }
}

async fn test_provider(config: ProviderTestConfig) -> Result<()> {
    let name = config.name;

    if config.skip {
        TEST_REPORT.record_skip(name);
        return Ok(());
    }

    TEST_REPORT.record_fail(name);

    {
        let _lock = ENV_LOCK.lock().unwrap();
        load_env();
        if config
            .required_vars
            .iter()
            .any(|var| std::env::var(var).is_err())
        {
            println!("Skipping {} tests - credentials not configured", name);
            TEST_REPORT.record_skip(name);
            return Ok(());
        }
    }

    let run_test = |mode: GooseMode| ProviderFixture::setup(&config, mode);

    if run_test(GooseMode::Auto).await.is_err() {
        println!("Skipping {} tests - failed to create provider", name);
        TEST_REPORT.record_skip(name);
        return Ok(());
    }

    let result: Result<()> = async {
        run_test(GooseMode::Auto)
            .await?
            .test_model_listing()
            .await?;
        run_test(GooseMode::Auto)
            .await?
            .test_basic_response()
            .await?;
        if config.test_mcp_tools {
            run_test(GooseMode::Auto).await?.test_tool_usage().await?;
            run_test(GooseMode::Auto)
                .await?
                .test_image_content_support()
                .await?;
        }
        if config.model_switch_name.is_some() {
            run_test(GooseMode::Auto).await?.test_model_switch().await?;
        }
        if config.test_context_length_exceeded {
            run_test(GooseMode::Auto)
                .await?
                .test_context_length_exceeded_error()
                .await?;
        }
        if config.test_thinking_effort {
            run_test(GooseMode::Auto)
                .await?
                .test_thinking_effort()
                .await?;
        }
        if config.test_permissions {
            run_test(GooseMode::Approve)
                .await?
                .test_permission_allow()
                .await?;
            run_test(GooseMode::Approve)
                .await?
                .test_permission_deny()
                .await?;
            if config.test_smart_approve {
                run_test(GooseMode::SmartApprove)
                    .await?
                    .test_smart_approve_llm_detect()
                    .await?;
                run_test(GooseMode::SmartApprove)
                    .await?
                    .test_smart_approve_readonly()
                    .await?;
            }
        }
        if config.test_mode_update {
            run_test(GooseMode::Auto).await?.test_mode_update().await?;
        }
        Ok(())
    }
    .await;

    match result {
        Ok(_) => {
            TEST_REPORT.record_pass(name);
            Ok(())
        }
        Err(e) => {
            println!("{} test failed: {}", name, e);
            TEST_REPORT.record_fail(name);
            Err(e)
        }
    }
}

#[tokio::test]
async fn test_openai_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider("openai", OPEN_AI_DEFAULT_MODEL, &["OPENAI_API_KEY"])
        .run()
        .await
}

#[tokio::test]
async fn test_azure_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "Azure",
        AZURE_DEFAULT_MODEL,
        &[
            "AZURE_OPENAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
            "AZURE_OPENAI_DEPLOYMENT_NAME",
        ],
    )
    .run()
    .await
}

#[cfg(feature = "aws-providers")]
#[tokio::test]
async fn test_bedrock_provider_long_term_credentials() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "aws_bedrock",
        BEDROCK_DEFAULT_MODEL,
        &["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"],
    )
    .run()
    .await
}

#[cfg(feature = "aws-providers")]
#[tokio::test]
async fn test_bedrock_provider_aws_profile_credentials() -> Result<()> {
    ProviderTestConfig::with_llm_provider("aws_bedrock", BEDROCK_DEFAULT_MODEL, &["AWS_PROFILE"])
        .clear_env(&["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"])
        .run()
        .await
}

#[cfg(feature = "aws-providers")]
#[tokio::test]
async fn test_bedrock_provider_bearer_token() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "aws_bedrock",
        BEDROCK_DEFAULT_MODEL,
        &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
    )
    .clear_env(&["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_PROFILE"])
    .run()
    .await
}

#[tokio::test]
async fn test_databricks_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "Databricks",
        DATABRICKS_DEFAULT_MODEL,
        &["DATABRICKS_HOST", "DATABRICKS_TOKEN"],
    )
    .run()
    .await
}

#[tokio::test]
async fn test_ollama_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider("Ollama", "qwen3", &["OLLAMA_HOST"])
        .image_model("qwen3-vl")
        // Above qwen3's 40960 context_length but small enough for Ollama's 600s timeout
        .context_length_exceeded(50_000)
        .expect_context_length_exceeded(false)
        .test_smart_approve(false)
        .run()
        .await
}

#[tokio::test]
async fn test_anthropic_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "Anthropic",
        ANTHROPIC_DEFAULT_MODEL,
        &["ANTHROPIC_API_KEY"],
    )
    .run()
    .await
}

#[tokio::test]
async fn test_openrouter_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "OpenRouter",
        OPEN_AI_DEFAULT_MODEL,
        &["OPENROUTER_API_KEY"],
    )
    .expect_context_length_exceeded(false)
    .run()
    .await
}

#[tokio::test]
async fn test_google_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider("Google", GOOGLE_DEFAULT_MODEL, &["GOOGLE_API_KEY"])
        .context_length_exceeded(2_600_000)
        .run()
        .await
}

#[tokio::test]
async fn test_snowflake_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "Snowflake",
        SNOWFLAKE_DEFAULT_MODEL,
        &["SNOWFLAKE_HOST", "SNOWFLAKE_TOKEN"],
    )
    .run()
    .await
}

#[cfg(feature = "aws-providers")]
#[tokio::test]
async fn test_sagemaker_tgi_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider(
        "SageMakerTgi",
        SAGEMAKER_TGI_DEFAULT_MODEL,
        &["SAGEMAKER_ENDPOINT_NAME"],
    )
    .run()
    .await
}

#[tokio::test]
async fn test_litellm_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider("LiteLLM", LITELLM_DEFAULT_MODEL, &["LITELLM_HOST"])
        .run()
        .await
}

#[tokio::test]
async fn test_xai_provider() -> Result<()> {
    ProviderTestConfig::with_llm_provider("Xai", XAI_DEFAULT_MODEL, &["XAI_API_KEY"])
        .run()
        .await
}

#[tokio::test]
async fn test_claude_code_provider() -> Result<()> {
    ProviderTestConfig::with_agentic_provider("claude-code", CLAUDE_CODE_DEFAULT_MODEL, "claude")
        .model_switch_name("sonnet")
        .run()
        .await
}

#[tokio::test]
async fn test_codex_provider() -> Result<()> {
    ProviderTestConfig::with_agentic_provider("codex", CODEX_DEFAULT_MODEL, "codex")
        .test_permissions(false)
        .run()
        .await
}

// Requires: npm install -g @agentclientprotocol/claude-agent-acp
#[tokio::test]
async fn test_claude_acp_provider() -> Result<()> {
    ProviderTestConfig::with_agentic_provider("claude-acp", ACP_CURRENT_MODEL, "claude-agent-acp")
        .model_switch_name("sonnet")
        .test_thinking_effort(true)
        .run()
        .await
}

// Requires: npm install -g @agentclientprotocol/codex-acp
#[tokio::test]
async fn test_codex_acp_provider() -> Result<()> {
    ProviderTestConfig::with_agentic_provider("codex-acp", ACP_CURRENT_MODEL, "codex-acp")
        .model_switch_name("gpt-5.4-mini")
        .run()
        .await
}

// Requires: npm install -g @github/copilot
#[tokio::test]
async fn test_copilot_acp_provider() -> Result<()> {
    ProviderTestConfig::with_agentic_provider("copilot-acp", ACP_CURRENT_MODEL, "copilot")
        .model_switch_name("gpt-4.1")
        // Copilot ignores mcpServers passed via session/new
        // https://github.com/github/copilot-cli/issues/1040
        .test_mcp_tools(false)
        .run()
        .await
}

#[dtor::dtor(unsafe)]
fn print_test_report() {
    TEST_REPORT.print_summary();
}
