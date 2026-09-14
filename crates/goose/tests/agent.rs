use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use goose::agents::{Agent, AgentEvent, GoosePlatform};
use goose::config::extensions::{set_extension, ExtensionEntry};

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(test)]
    mod schedule_tool_tests {
        use super::*;
        use async_trait::async_trait;
        use chrono::{DateTime, Utc};
        use goose::agents::platform_extensions::scheduler::{
            EXTENSION_NAME as SCHEDULER_EXTENSION_NAME, MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        };
        use goose::agents::ExtensionConfig;
        use goose::agents::{AgentConfig, ScheduleTool};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::scheduler::{ScheduledJob, SchedulerError, ValidatedScheduleRecipe};
        use goose::scheduler_trait::SchedulerTrait;
        use goose::session::{Session, SessionManager};
        use std::path::PathBuf;
        use std::sync::Arc;
        use tempfile::TempDir;

        struct MockScheduler {
            jobs: tokio::sync::Mutex<Vec<ScheduledJob>>,
        }

        struct SessionsMockScheduler {
            sessions: Vec<(String, Session)>,
        }

        impl SessionsMockScheduler {
            fn new(sessions: Vec<(String, Session)>) -> Self {
                Self { sessions }
            }
        }

        #[async_trait]
        impl SchedulerTrait for SessionsMockScheduler {
            async fn add_scheduled_job(
                &self,
                _job: ScheduledJob,
                _copy: bool,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn add_scheduled_job_with_recipe(
                &self,
                _job: ScheduledJob,
                _validated_recipe: ValidatedScheduleRecipe,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn schedule_recipe(
                &self,
                _recipe_path: PathBuf,
                _cron_schedule: Option<String>,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn list_scheduled_jobs(&self) -> Vec<ScheduledJob> {
                Vec::new()
            }

            async fn remove_scheduled_job(
                &self,
                _id: &str,
                _remove: bool,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn pause_schedule(&self, _id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn unpause_schedule(&self, _id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn run_now(&self, _id: &str) -> Result<String, SchedulerError> {
                Ok("test_session_123".to_string())
            }

            async fn sessions(
                &self,
                _sched_id: &str,
                _limit: usize,
            ) -> Result<Vec<(String, Session)>, SchedulerError> {
                Ok(self.sessions.clone())
            }

            async fn update_schedule(
                &self,
                _sched_id: &str,
                _new_cron: String,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn kill_running_job(&self, _sched_id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn get_running_job_info(
                &self,
                _sched_id: &str,
            ) -> Result<Option<(String, DateTime<Utc>)>, SchedulerError> {
                Ok(None)
            }
        }

        impl MockScheduler {
            fn new() -> Self {
                Self {
                    jobs: tokio::sync::Mutex::new(Vec::new()),
                }
            }
        }

        async fn add_scheduler_extension(agent: &Agent) {
            agent
                .extension_manager
                .add_extension(
                    ExtensionConfig::Platform {
                        name: SCHEDULER_EXTENSION_NAME.to_string(),
                        description: "Create and manage scheduled recipe execution".to_string(),
                        display_name: Some("Scheduler".to_string()),
                        bundled: Some(true),
                        available_tools: vec![],
                    },
                    None,
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        #[async_trait]
        impl SchedulerTrait for MockScheduler {
            async fn add_scheduled_job(
                &self,
                job: ScheduledJob,
                _copy: bool,
            ) -> Result<(), SchedulerError> {
                let mut jobs = self.jobs.lock().await;
                jobs.push(job);
                Ok(())
            }

            async fn add_scheduled_job_with_recipe(
                &self,
                job: ScheduledJob,
                _validated_recipe: ValidatedScheduleRecipe,
            ) -> Result<(), SchedulerError> {
                let mut jobs = self.jobs.lock().await;
                jobs.push(job);
                Ok(())
            }

            async fn schedule_recipe(
                &self,
                _recipe_path: PathBuf,
                _cron_schedule: Option<String>,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn list_scheduled_jobs(&self) -> Vec<ScheduledJob> {
                let jobs = self.jobs.lock().await;
                jobs.clone()
            }

            async fn remove_scheduled_job(
                &self,
                id: &str,
                _remove: bool,
            ) -> Result<(), SchedulerError> {
                let mut jobs = self.jobs.lock().await;
                if let Some(pos) = jobs.iter().position(|job| job.id == id) {
                    jobs.remove(pos);
                    Ok(())
                } else {
                    Err(SchedulerError::JobNotFound(id.to_string()))
                }
            }

            async fn pause_schedule(&self, _id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn unpause_schedule(&self, _id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn run_now(&self, _id: &str) -> Result<String, SchedulerError> {
                Ok("test_session_123".to_string())
            }

            async fn sessions(
                &self,
                _sched_id: &str,
                _limit: usize,
            ) -> Result<Vec<(String, Session)>, SchedulerError> {
                Ok(vec![])
            }

            async fn update_schedule(
                &self,
                _sched_id: &str,
                _new_cron: String,
            ) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn kill_running_job(&self, _sched_id: &str) -> Result<(), SchedulerError> {
                Ok(())
            }

            async fn get_running_job_info(
                &self,
                _sched_id: &str,
            ) -> Result<Option<(String, DateTime<Utc>)>, SchedulerError> {
                Ok(None)
            }
        }

        #[tokio::test]
        async fn test_schedule_management_tool_list() {
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
            let permission_manager = Arc::new(PermissionManager::new(data_dir));
            let mock_scheduler = Arc::new(MockScheduler::new());
            let config = AgentConfig::new(
                session_manager,
                permission_manager,
                Some(mock_scheduler),
                GooseMode::Auto,
                false,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            add_scheduler_extension(&agent).await;

            let tools = agent.list_tools("test-session-id", None).await;
            let schedule_tool = tools
                .iter()
                .find(|tool| tool.name == MANAGE_SCHEDULE_TOOL_NAME_COMPLETE);
            assert!(schedule_tool.is_some());

            let tool = schedule_tool.unwrap();
            assert!(tool
                .description
                .clone()
                .unwrap_or_default()
                .contains("Manage goose's internal scheduled recipe execution"));
        }

        #[tokio::test]
        async fn test_no_schedule_management_tool_without_scheduler() {
            let agent = Agent::new();
            add_scheduler_extension(&agent).await;

            let tools = agent.list_tools("test-session-id", None).await;
            let schedule_tool = tools
                .iter()
                .find(|tool| tool.name == MANAGE_SCHEDULE_TOOL_NAME_COMPLETE);
            assert!(schedule_tool.is_none());
        }

        #[tokio::test]
        async fn test_schedule_management_tool_in_scheduler_extension() {
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
            let permission_manager = Arc::new(PermissionManager::new(data_dir));
            let mock_scheduler = Arc::new(MockScheduler::new());
            let config = AgentConfig::new(
                session_manager,
                permission_manager,
                Some(mock_scheduler),
                GooseMode::Auto,
                false,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            add_scheduler_extension(&agent).await;

            let tools = agent
                .list_tools(
                    "test-session-id",
                    Some(SCHEDULER_EXTENSION_NAME.to_string()),
                )
                .await;

            let schedule_tool = tools
                .iter()
                .find(|tool| tool.name == MANAGE_SCHEDULE_TOOL_NAME_COMPLETE);
            assert!(schedule_tool.is_some());

            let tool = schedule_tool.unwrap();
            assert!(tool
                .description
                .clone()
                .unwrap_or_default()
                .contains("Manage goose's internal scheduled recipe execution"));

            // Verify the tool has the expected actions in its schema
            if let Some(properties) = tool.input_schema.get("properties") {
                if let Some(action_prop) = properties.get("action") {
                    if let Some(enum_values) = action_prop.get("enum") {
                        let actions: Vec<String> = enum_values
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap().to_string())
                            .collect();

                        // Check that our session_content action is included
                        assert!(actions.contains(&"session_content".to_string()));
                        assert!(actions.contains(&"list".to_string()));
                        assert!(actions.contains(&"create".to_string()));
                        assert!(actions.contains(&"sessions".to_string()));
                    }
                }
            }
        }

        #[tokio::test]
        async fn test_schedule_management_tool_schema_validation() {
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
            let permission_manager = Arc::new(PermissionManager::new(data_dir));
            let mock_scheduler = Arc::new(MockScheduler::new());
            let config = AgentConfig::new(
                session_manager,
                permission_manager,
                Some(mock_scheduler),
                GooseMode::Auto,
                false,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            add_scheduler_extension(&agent).await;

            let tools = agent.list_tools("test-session-id", None).await;
            let schedule_tool = tools
                .iter()
                .find(|tool| tool.name == MANAGE_SCHEDULE_TOOL_NAME_COMPLETE);
            assert!(schedule_tool.is_some());

            let tool = schedule_tool.unwrap();

            // Verify the tool schema has the session_id parameter for session_content action
            if let Some(properties) = tool.input_schema.get("properties") {
                assert!(properties.get("session_id").is_some());

                if let Some(session_id_prop) = properties.get("session_id") {
                    assert_eq!(
                        session_id_prop.get("type").unwrap().as_str().unwrap(),
                        "string"
                    );
                    assert!(session_id_prop
                        .get("description")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .contains("Session identifier for session_content action"));
                }
            }
        }

        #[tokio::test]
        async fn test_schedule_sessions_reports_message_count_without_conversation() {
            let temp_dir = TempDir::new().unwrap();
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));

            let session = Session {
                id: "session-123".to_string(),
                message_count: 37,
                conversation: None,
                ..Default::default()
            };

            let mock_scheduler = Arc::new(SessionsMockScheduler::new(vec![(
                "session-123".to_string(),
                session,
            )]));
            let schedule_tool = ScheduleTool::new(mock_scheduler, session_manager);

            let result = schedule_tool
                .execute(serde_json::json!({
                    "action": "sessions",
                    "job_id": "daily-report"
                }))
                .await
                .expect("schedule sessions should succeed");

            let text = result
                .into_iter()
                .filter_map(|content| match content {
                    rmcp::model::ContentBlock::Text(text_content) => {
                        Some(text_content.text.clone())
                    }
                    _ => None,
                })
                .collect::<String>();
            assert!(
                text.contains("Messages: 37"),
                "expected stored message_count in sessions output, got: {text}"
            );
        }
    }

    #[cfg(test)]
    mod retry_tests {
        use super::*;
        use goose::agents::types::{RetryConfig, SuccessCheck};

        #[tokio::test]
        async fn test_retry_success_check_execution() -> Result<()> {
            use goose::agents::retry::execute_success_checks;

            let retry_config = RetryConfig {
                max_retries: 3,
                checks: vec![],
                on_failure: None,
                timeout_seconds: Some(30),
                on_failure_timeout_seconds: Some(60),
            };

            let success_checks = vec![SuccessCheck::Shell {
                command: "echo 'test'".to_string(),
            }];

            let result = execute_success_checks(&success_checks, &retry_config).await;
            assert!(result.is_ok(), "Success check should pass");
            assert!(result.unwrap(), "Command should succeed");

            let fail_checks = vec![SuccessCheck::Shell {
                command: "false".to_string(),
            }];

            let result = execute_success_checks(&fail_checks, &retry_config).await;
            assert!(result.is_ok(), "Success check execution should not error");
            assert!(!result.unwrap(), "Command should fail");

            Ok(())
        }

        #[tokio::test]
        async fn test_retry_logic_with_validation_errors() -> Result<()> {
            let invalid_retry_config = RetryConfig {
                max_retries: 0,
                checks: vec![],
                on_failure: None,
                timeout_seconds: Some(0),
                on_failure_timeout_seconds: None,
            };

            let validation_result = invalid_retry_config.validate();
            assert!(
                validation_result.is_err(),
                "Should validate max_retries > 0"
            );
            assert!(validation_result
                .unwrap_err()
                .contains("max_retries must be greater than 0"));

            Ok(())
        }

        #[tokio::test]
        async fn test_retry_attempts_counter_reset() -> Result<()> {
            let agent = Agent::new();

            agent.reset_retry_attempts().await;
            let initial_attempts = agent.get_retry_attempts().await;
            assert_eq!(initial_attempts, 0);

            let new_attempts = agent.increment_retry_attempts().await;
            assert_eq!(new_attempts, 1);

            agent.reset_retry_attempts().await;
            let reset_attempts = agent.get_retry_attempts().await;
            assert_eq!(reset_attempts, 0);

            Ok(())
        }
    }

    #[cfg(test)]
    mod max_turns_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::SessionConfig;
        use goose::config::GooseMode;
        use goose::conversation::message::{Message, MessageContent};
        use goose::providers::base::{
            stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
        };
        use goose::session::session_manager::SessionType;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{CallToolRequestParams, Tool};
        use rmcp::object;
        use std::path::PathBuf;

        struct MockToolProvider {}

        impl MockToolProvider {
            fn new() -> Self {
                Self {}
            }
        }

        impl goose::providers::base::ProviderDescriptor for MockToolProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "mock".to_string(),
                    display_name: "Mock Provider".to_string(),
                    description: "Mock provider for testing".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for MockToolProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                Box::pin(async { Ok(Self::new()) })
            }
        }

        #[async_trait]
        impl Provider for MockToolProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let tool_call = CallToolRequestParams::new("test_tool")
                    .with_arguments(object!({"param": "value"}));
                let message = Message::assistant().with_tool_request("call_123", Ok(tool_call));

                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(5), Some(15)),
                );

                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "mock-test"
            }
        }

        #[tokio::test]
        async fn test_max_turns_limit() -> Result<()> {
            let agent = Agent::new();
            let provider = Arc::new(MockToolProvider::new());
            let user_message = Message::user().with_text("Hello");

            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "max-turn-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
                .await?;

            let session_id = session.id;
            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(1),
                retry_config: None,
            };

            let reply_stream = agent.reply(user_message, session_config, None).await?;
            tokio::pin!(reply_stream);

            let mut responses = Vec::new();
            while let Some(response_result) = reply_stream.next().await {
                match response_result {
                    Ok(AgentEvent::Message(response)) => {
                        if let Some(MessageContent::ActionRequired(action)) =
                            response.content.first()
                        {
                            if let goose::conversation::message::ActionRequiredData::ToolConfirmation { id, .. } = &action.data {
                                agent
                                    .submit_tool_confirmation(
                                        &session_id,
                                        id,
                                        goose::permission::Permission::AllowOnce,
                                    )
                                    .await?;
                            }
                        }
                        responses.push(response);
                    }
                    Ok(AgentEvent::McpNotification(_)) => {}
                    Ok(AgentEvent::Usage(_)) => {}
                    Ok(AgentEvent::MessageUsage { .. }) => {}
                    Ok(AgentEvent::HistoryReplaced(_updated_conversation)) => {
                        // We should update the conversation here, but we're not reading it
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }

            assert!(
                !responses.is_empty(),
                "Expected at least 1 response, got {}",
                responses.len()
            );

            // Look for the max turns message as the last response
            let last_response = responses.last().unwrap();
            let last_content = last_response.content.first().unwrap();
            if let MessageContent::Text(text_content) = last_content {
                assert!(text_content.text.contains(
                    "I've reached the maximum number of actions I can do without user input"
                ));
            } else {
                panic!("Expected text content in last message");
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod unparseable_tool_call_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::{Message, MessageContent};
        use goose::providers::base::{
            stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
        };
        use goose::session::session_manager::SessionType;
        use goose::session::SessionManager;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{ErrorCode, ErrorData, Tool};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tempfile::TempDir;

        /// First turn returns a tool request that failed to parse (mirroring what
        /// the decoders emit for non-object arguments), subsequent turns return
        /// plain text so the loop can finish.
        struct UnparseableToolProvider {
            call_count: AtomicUsize,
        }

        impl UnparseableToolProvider {
            fn new() -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for UnparseableToolProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "mock-unparseable".to_string(),
                    display_name: "Mock Unparseable Provider".to_string(),
                    description: "Mock provider for unparseable tool call tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for UnparseableToolProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                Box::pin(async { Ok(Self::new()) })
            }
        }

        #[async_trait]
        impl Provider for UnparseableToolProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst);
                let message = if n == 0 {
                    let error = ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Tool arguments must be a JSON object".to_string(),
                        None,
                    );
                    Message::assistant().with_tool_request("call_bad", Err(error))
                } else {
                    Message::assistant().with_text("Recovered after the bad tool call.")
                };

                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(5), Some(15)),
                );
                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "mock-unparseable"
            }
        }

        /// An unparseable tool call should be fed back to the model as a tool
        /// response error so it can retry, rather than terminating the run.
        #[tokio::test]
        async fn test_unparseable_tool_call_feeds_back_and_continues() -> Result<()> {
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
            let agent = Agent::with_config(AgentConfig::new(
                session_manager.clone(),
                Arc::new(PermissionManager::new(data_dir)),
                None,
                GooseMode::default(),
                true,
                GoosePlatform::GooseCli,
            ));
            let provider = Arc::new(UnparseableToolProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "unparseable-tool-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(5),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hello"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut saw_tool_response_error = false;
            let mut saw_recovery_text = false;
            while let Some(event) = reply_stream.next().await {
                if let Ok(AgentEvent::Message(message)) = event {
                    for content in &message.content {
                        match content {
                            MessageContent::ToolResponse(response)
                                if response.id == "call_bad" && response.tool_result.is_err() =>
                            {
                                saw_tool_response_error = true;
                            }
                            MessageContent::Text(text)
                                if text.text.contains("Recovered after the bad tool call") =>
                            {
                                saw_recovery_text = true;
                            }
                            _ => {}
                        }
                    }
                }
            }

            assert!(
                saw_tool_response_error,
                "expected an error tool response fed back to the model for the unparseable call"
            );
            assert!(
                saw_recovery_text,
                "expected the loop to continue to a second provider turn instead of terminating"
            );
            assert!(
                provider.call_count.load(Ordering::SeqCst) >= 2,
                "provider should have been called again after the bad tool call"
            );
            Ok(())
        }
    }

    #[cfg(test)]
    mod tool_pair_summarization_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::base::Config;
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::Message;
        use goose::providers::base::{
            stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
        };
        use goose::session::{SessionManager, SessionType};
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        /// Mock provider that returns text for the main reply and summaries for
        /// summarization calls. Distinguishes by checking if tools are empty
        /// (summarization calls pass no tools).
        struct SummarizationTestProvider {
            summary_count: AtomicUsize,
        }

        impl SummarizationTestProvider {
            fn new() -> Self {
                Self {
                    summary_count: AtomicUsize::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for SummarizationTestProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "mock-summarization".to_string(),
                    display_name: "Mock Summarization Provider".to_string(),
                    description: "Mock provider for summarization tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for SummarizationTestProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                Box::pin(async { Ok(Self::new()) })
            }
        }

        #[async_trait]
        impl Provider for SummarizationTestProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let message = if system_prompt.contains("summarize a tool call") {
                    // Summarization call — return a unique summary
                    let n = self.summary_count.fetch_add(1, Ordering::SeqCst);
                    Message::assistant().with_text(format!("Summary of tool call #{}", n))
                } else {
                    // Main agent reply — return plain text so the loop exits
                    Message::assistant().with_text("Done processing.")
                };

                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(5), Some(15)),
                );
                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "mock-summarization"
            }
        }

        /// Test that batch tool pair summarization preserves all summaries.
        ///
        /// Pre-populates a session with enough tool call/response pairs to trigger
        /// batch summarization, runs agent.reply(), then verifies:
        /// - All 10 summaries are present in the final conversation
        /// - The original tool pairs are marked invisible
        #[tokio::test]
        async fn test_batch_summarization_preserves_all_summaries() -> Result<()> {
            // Set a low cutoff so we don't need hundreds of tool pairs.
            // cutoff=2 means we need >2+10=12 visible tool pairs to trigger.
            Config::global()
                .set_param("GOOSE_TOOL_CALL_CUTOFF", 2)
                .unwrap();

            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().join("data")));
            let agent = Agent::with_config(AgentConfig::new(
                Arc::clone(&session_manager),
                Arc::new(PermissionManager::new(temp_dir.path().join("config"))),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            ));
            let provider = Arc::new(SummarizationTestProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::from("."),
                    "summarization-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::Auto,
                )
                .await?;

            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
                .await?;

            // Pre-populate 13 tool pairs (need > cutoff + batch_size = 12 to trigger).
            // Timestamps in the past so DB ordering places summaries before current turn.
            let base_ts = chrono::Utc::now().timestamp() - 100;

            let mut initial_msg = Message::user().with_text("help me read some files");
            initial_msg.created = base_ts;
            session_manager
                .add_message(&session.id, &initial_msg)
                .await?;

            for i in 0..13 {
                let call_id = format!("precall_{}", i);
                let mut req_msg = Message::assistant()
                    .with_tool_request(&call_id, Ok(CallToolRequestParams::new("read_file")))
                    .with_generated_id();
                req_msg.created = base_ts + i as i64 + 1;
                session_manager.add_message(&session.id, &req_msg).await?;

                let mut resp_msg = Message::user()
                    .with_tool_response(
                        &call_id,
                        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                            "content of file {}",
                            i
                        ))])),
                    )
                    .with_generated_id();
                resp_msg.created = base_ts + i as i64 + 1;
                session_manager.add_message(&session.id, &resp_msg).await?;
            }

            // Send a user message to trigger the reply loop
            let user_message = Message::user().with_text("summarize what you found");

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(1),
                retry_config: None,
            };

            let reply_stream = agent.reply(user_message, session_config, None).await?;
            tokio::pin!(reply_stream);

            // Drain the stream
            while let Some(event) = reply_stream.next().await {
                match event {
                    Ok(AgentEvent::Message(_)) => {}
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            // Load the final session and inspect the conversation
            let final_session = session_manager.get_session(&session.id, true).await?;
            let conversation = final_session
                .conversation
                .expect("Session should have a conversation");
            let messages = conversation.messages();

            // Count summaries: messages that are agent-visible, not user-visible,
            // and contain our summary text pattern
            let summaries: Vec<&Message> = messages
                .iter()
                .filter(|m| {
                    m.metadata.agent_visible
                        && !m.metadata.user_visible
                        && m.as_concat_text().starts_with("Summary of tool call #")
                })
                .collect();

            assert_eq!(
                summaries.len(),
                10,
                "Expected 10 summaries (one full batch), got {}. Summary texts: {:?}",
                summaries.len(),
                summaries
                    .iter()
                    .map(|m| m.as_concat_text())
                    .collect::<Vec<_>>()
            );

            // Verify each summary is unique
            let summary_texts: std::collections::HashSet<String> =
                summaries.iter().map(|m| m.as_concat_text()).collect();
            assert_eq!(summary_texts.len(), 10, "All 10 summaries should be unique");

            // Count invisible tool pairs: original pairs that were summarized
            // should have agent_visible=false
            let invisible_tool_msgs: Vec<&Message> = messages
                .iter()
                .filter(|m| !m.metadata.agent_visible && (m.is_tool_call() || m.is_tool_response()))
                .collect();

            // Each summarized pair = 2 invisible messages (request + response)
            assert_eq!(
                invisible_tool_msgs.len(),
                20, // 10 pairs × 2 messages
                "Expected 20 invisible tool messages (10 summarized pairs), got {}",
                invisible_tool_msgs.len()
            );

            // Summaries must appear before the current turn's reply, not after it
            let agent_visible: Vec<&Message> = messages
                .iter()
                .filter(|m| m.metadata.agent_visible)
                .collect();

            let last_summary_pos = agent_visible
                .iter()
                .rposition(|m| m.as_concat_text().starts_with("Summary of tool call #"))
                .expect("Should have at least one summary");
            let agent_reply_pos = agent_visible
                .iter()
                .position(|m| m.as_concat_text().contains("Done processing."))
                .expect("Should have the agent reply");

            assert!(
                last_summary_pos < agent_reply_pos,
                "Summaries appeared after the current turn's reply: last_summary={}, reply={}",
                last_summary_pos,
                agent_reply_pos,
            );

            // Clean up the config override
            Config::global().delete("GOOSE_TOOL_CALL_CUTOFF").unwrap();

            Ok(())
        }
    }

    #[cfg(test)]
    mod extension_manager_tests {
        use super::*;
        use goose::agents::extension::ExtensionConfig;
        use goose::agents::platform_extensions::{
            MANAGE_EXTENSIONS_TOOL_NAME, SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME,
        };
        use goose::agents::AgentConfig;
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::session::SessionManager;

        async fn setup_agent_with_extension_manager() -> (Agent, String) {
            use goose::session::session_manager::SessionType;

            // Add the TODO extension to the config so it can be discovered by search_available_extensions
            // Set it as disabled initially so tests can enable it
            let todo_extension_entry = ExtensionEntry {
                enabled: false,
                config: ExtensionConfig::Platform {
                    name: "todo".to_string(),
                    description:
                        "Enable a todo list for goose so it can keep track of what it is doing"
                            .to_string(),
                    display_name: Some("Todo".to_string()),
                    bundled: Some(true),
                    available_tools: vec![],
                },
            };
            set_extension(todo_extension_entry);

            // Create agent with session_id from the start
            let temp_dir = tempfile::tempdir().unwrap();
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::default(),
                false,
                GoosePlatform::GooseCli,
            );

            let agent = Agent::with_config(config);

            let session = session_manager
                .create_session(
                    std::path::PathBuf::from("."),
                    "Test Session".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await
                .expect("Failed to create session");
            let session_id = session.id;

            // Now add the extension manager platform extension
            let ext_config = ExtensionConfig::Platform {
                name: "extensionmanager".to_string(),
                description: "Extension Manager".to_string(),
                display_name: Some("Extension Manager".to_string()),
                bundled: Some(true),
                available_tools: vec![],
            };

            agent
                .add_extension(ext_config, &session_id)
                .await
                .expect("Failed to add extension manager");
            (agent, session_id)
        }

        #[tokio::test]
        async fn test_extension_manager_tools_available() {
            let (agent, session_id) = setup_agent_with_extension_manager().await;
            let tools = agent.list_tools(&session_id, None).await;

            // Note: Tool names are prefixed with the normalized extension name "extensionmanager"
            // not the display name "Extension Manager"
            let search_tool = tools.iter().find(|tool| {
                tool.name == format!("extensionmanager__{SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME}")
            });
            assert!(
                search_tool.is_some(),
                "search_available_extensions tool should be available"
            );

            let manage_tool = tools.iter().find(|tool| {
                tool.name == format!("extensionmanager__{MANAGE_EXTENSIONS_TOOL_NAME}")
            });
            assert!(
                manage_tool.is_some(),
                "manage_extensions tool should be available"
            );
        }
    }

    #[cfg(test)]
    mod streaming_persistence_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::Message;
        use goose::providers::base::{MessageStream, Provider, ProviderDef, ProviderMetadata};
        use goose::session::session_manager::SessionType;
        use goose::session::SessionManager;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{CallToolRequestParams, Role, Tool};
        use rmcp::object;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio_util::sync::CancellationToken;

        struct MultiStepProvider {
            call_count: AtomicUsize,
            cancel_token: CancellationToken,
        }

        impl MultiStepProvider {
            fn new(cancel_token: CancellationToken) -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                    cancel_token,
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for MultiStepProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "multi-step-mock".to_string(),
                    display_name: "Multi-Step Mock".to_string(),
                    description: "Mock provider for streaming persistence tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for MultiStepProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for MultiStepProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(5), Some(15)),
                );

                match call {
                    0 => {
                        let tool_call = CallToolRequestParams::new("test_tool")
                            .with_arguments(object!({"param": "value"}));
                        let message =
                            Message::assistant().with_tool_request("call_1", Ok(tool_call));
                        let stream =
                            futures::stream::once(async move { Ok((Some(message), Some(usage))) });
                        Ok(Box::pin(stream))
                    }
                    1 => {
                        let msg_id = format!("msg_{}", uuid::Uuid::new_v4());
                        let tokens = vec!["Hello", " world", ", how", " are", " you?"];
                        let stream = futures::stream::iter(tokens.into_iter().enumerate().map(
                            move |(i, token)| {
                                let msg = Message::assistant()
                                    .with_text(token)
                                    .with_id(msg_id.clone());
                                let u = if i == 4 { Some(usage.clone()) } else { None };
                                Ok((Some(msg), u))
                            },
                        ));
                        Ok(Box::pin(stream))
                    }
                    _ => {
                        let cancel = self.cancel_token.clone();
                        let msg_id = format!("msg_{}", uuid::Uuid::new_v4());
                        let tokens = vec!["This ", "should ", "be ", "cancelled ", "soon."];
                        let stream = futures::stream::iter(tokens.into_iter().enumerate().map(
                            move |(i, token)| {
                                if i == 1 {
                                    cancel.cancel();
                                }
                                let msg = Message::assistant()
                                    .with_text(token)
                                    .with_id(msg_id.clone());
                                let u = if i == 4 { Some(usage.clone()) } else { None };
                                Ok((Some(msg), u))
                            },
                        ));
                        Ok(Box::pin(stream))
                    }
                }
            }

            fn get_name(&self) -> &str {
                "multi-step-mock"
            }
        }

        #[tokio::test]
        async fn test_streaming_text_not_persisted_per_token() -> Result<()> {
            let cancel_token = CancellationToken::new();
            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true, // disable session naming so it doesn't consume a provider call
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(MultiStepProvider::new(cancel_token.clone()));

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "streaming-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session_id)
                .await?;

            // ── Single reply: tool call (call 0) → text stream (call 1) → cancelled text (call 2)
            // max_turns=3 allows all three provider calls within one reply().
            //   call 0: tool call → agent executes tool, loops
            //   call 1: 5 text deltas → no tools called, agent exits loop
            //   call 2: 5 text deltas, cancel token fired after 1st → agent interrupted
            //
            // Because call 1 ends the agent loop (no_tools_called=true → exit),
            // call 2 is NOT reached in the same reply. We issue a second reply()
            // with the cancel token so the provider triggers cancellation.
            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Do something then say hello"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);

            while let Some(event) = reply_stream.next().await {
                match event {
                    Ok(AgentEvent::Message(_)) => {}
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            // ── Check persisted state after reply 1 ─────────────────
            let reloaded = session_manager.get_session(&session_id, true).await?;
            let messages = reloaded
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec();

            // Turn-context events are excluded; this test is about streaming deltas.
            let user_count = messages
                .iter()
                .filter(|m| m.role == Role::User && m.is_user_visible())
                .count();
            let asst_count = messages
                .iter()
                .filter(|m| m.role == Role::Assistant)
                .count();

            // Expected: user(prompt) + assistant(tool-req) + user(tool-resp) + assistant(text)
            assert_eq!(
                user_count, 2,
                "Expected 2 user messages (prompt + tool response), got {user_count}",
            );
            assert_eq!(
                asst_count, 2,
                "Expected 2 assistant messages (tool request + text reply), got {asst_count} \
                 — streaming text deltas are being persisted as separate messages",
            );

            // ── Reply 2: text stream with provider-triggered cancellation (call 2)
            let session_config2 = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream2 = agent
                .reply(
                    Message::user().with_text("Tell me more"),
                    session_config2,
                    Some(cancel_token),
                )
                .await?;
            tokio::pin!(reply_stream2);

            while let Some(event) = reply_stream2.next().await {
                match event {
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            // ── Check persisted state after cancellation ────────────
            let reloaded2 = session_manager.get_session(&session_id, true).await?;
            let messages2 = reloaded2
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec();

            let user_count2 = messages2
                .iter()
                .filter(|m| m.role == Role::User && m.is_user_visible())
                .count();
            let asst_count2 = messages2
                .iter()
                .filter(|m| m.role == Role::Assistant)
                .count();

            // Reply 2 added 1 user message. The cancelled stream should
            // have persisted at most 1 (partial) assistant message.
            assert_eq!(
                user_count2, 3,
                "Expected 3 user messages (2 from reply 1 + follow-up), got {user_count2}",
            );
            assert!(
                asst_count2 <= 3,
                "Expected at most 3 assistant messages (2 from reply 1 + at most 1 partial \
                 from cancelled reply 2), got {asst_count2} \
                 — streaming deltas are leaking into persistence",
            );

            Ok(())
        }
    }

    #[cfg(test)]
    mod thinking_preservation_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::{Message, MessageContent};
        use goose::providers::base::{MessageStream, Provider, ProviderDef, ProviderMetadata};
        use goose::session::session_manager::SessionType;
        use goose::session::SessionManager;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{CallToolRequestParams, Tool};
        use rmcp::object;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Simulates DeepSeek/Kimi streaming: reasoning_content arrives in an early
        /// chunk, the tool call arrives in a later chunk with no reasoning_content.
        struct ThinkingStreamProvider {
            call_count: AtomicUsize,
            name: &'static str,
        }

        impl ThinkingStreamProvider {
            fn new(name: &'static str) -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                    name,
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for ThinkingStreamProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "thinking-stream-mock".to_string(),
                    display_name: "Thinking Stream Mock".to_string(),
                    description: "Mock for thinking preservation tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for ThinkingStreamProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for ThinkingStreamProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(20), Some(30)),
                );
                match call {
                    0 => {
                        // Chunk 1: reasoning_content only (no tool call)
                        let thinking =
                            Message::assistant().with_thinking("I should call test_tool", "sig_0");
                        // Chunk 2: tool call only (no reasoning_content) — the bug scenario
                        let tool_call = CallToolRequestParams::new("test_tool")
                            .with_arguments(object!({"param": "value"}));
                        let tool_msg =
                            Message::assistant().with_tool_request("call_1", Ok(tool_call));
                        let stream = futures::stream::iter(vec![
                            Ok((Some(thinking), None)),
                            Ok((Some(tool_msg), Some(usage))),
                        ]);
                        Ok(Box::pin(stream))
                    }
                    _ => {
                        let msg = Message::assistant().with_text("Done.");
                        Ok(Box::pin(futures::stream::once(async move {
                            Ok((Some(msg), Some(usage)))
                        })))
                    }
                }
            }

            fn get_name(&self) -> &str {
                self.name
            }
        }

        async fn run_and_collect(provider_name: &'static str) -> Result<Vec<Message>> {
            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(ThinkingStreamProvider::new(provider_name));

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    format!("{provider_name}-thinking-test"),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session_id)
                .await?;

            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Use the test tool"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);

            while let Some(event) = reply_stream.next().await {
                event?;
            }

            let reloaded = session_manager.get_session(&session_id, true).await?;
            Ok(reloaded
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec())
        }

        fn assert_formatter_adds_reasoning_to_tool_calls(messages: &[Message], provider: &str) {
            use goose_providers::formats::openai::{
                format_messages_with_options, OpenAiFormatOptions,
            };
            use goose_providers::images::ImageFormat;

            assert!(
                messages.iter().any(|m| m
                    .content
                    .iter()
                    .any(|c| matches!(c, MessageContent::Thinking(_)))),
                "{provider}: conversation must contain at least one Thinking message"
            );
            assert!(
                messages.iter().any(|m| m
                    .content
                    .iter()
                    .any(|c| matches!(c, MessageContent::ToolRequest(_)))),
                "{provider}: conversation must contain at least one tool-call message"
            );

            let spec = format_messages_with_options(
                messages,
                &ImageFormat::OpenAi,
                OpenAiFormatOptions {
                    preserve_thinking_context: true,
                    ..Default::default()
                },
            );
            let has_reasoning_on_tool_call = spec.iter().any(|m| {
                m.get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .is_some_and(|a| !a.is_empty())
                    && m.get("reasoning_content").is_some()
            });
            assert!(
                has_reasoning_on_tool_call,
                "{provider}: formatter must produce reasoning_content on assistant tool-call \
                 messages — {provider} returns HTTP 400 when it is absent on the next turn"
            );
        }

        /// DeepSeek streams reasoning_content before the tool-call chunk. The formatter
        /// must attach it to the tool-call message so the next turn is accepted.
        #[tokio::test]
        async fn test_deepseek_thinking_preserved_in_tool_call_message() -> Result<()> {
            let messages = run_and_collect("deepseek-mock").await?;
            assert_formatter_adds_reasoning_to_tool_calls(&messages, "DeepSeek");
            Ok(())
        }

        /// Kimi has the same streaming behaviour as DeepSeek.
        #[tokio::test]
        async fn test_kimi_thinking_preserved_in_tool_call_message() -> Result<()> {
            let messages = run_and_collect("kimi-mock").await?;
            assert_formatter_adds_reasoning_to_tool_calls(&messages, "Kimi");
            Ok(())
        }

        /// Simulates a provider that emits reasoning and a tool call in the same
        /// streamed message (no prior thinking-only chunk).
        struct CombinedThinkingToolProvider {
            call_count: AtomicUsize,
        }

        impl CombinedThinkingToolProvider {
            fn new() -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for CombinedThinkingToolProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "combined-thinking-tool-mock".to_string(),
                    display_name: "Combined Thinking+Tool Mock".to_string(),
                    description: "Mock for combined thinking+tool call in one chunk".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for CombinedThinkingToolProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for CombinedThinkingToolProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(20), Some(30)),
                );
                match call {
                    0 => {
                        // Single chunk: reasoning_content AND tool call together
                        let tool_call = CallToolRequestParams::new("test_tool")
                            .with_arguments(object!({"param": "value"}));
                        let combined = Message::assistant()
                            .with_thinking("I should call test_tool", "sig_0")
                            .with_tool_request("call_1", Ok(tool_call));
                        Ok(Box::pin(futures::stream::once(async move {
                            Ok((Some(combined), Some(usage)))
                        })))
                    }
                    _ => {
                        let msg = Message::assistant().with_text("Done.");
                        Ok(Box::pin(futures::stream::once(async move {
                            Ok((Some(msg), Some(usage)))
                        })))
                    }
                }
            }

            fn get_name(&self) -> &str {
                "combined-thinking-tool-mock"
            }
        }

        /// When reasoning arrives in the same chunk as the tool call (no prior
        /// thinking-only message), the agent must attach it to the persisted
        /// request_msg so the formatter can emit reasoning_content on the next turn.
        #[tokio::test]
        async fn test_reasoning_preserved_when_combined_with_tool_call() -> Result<()> {
            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(CombinedThinkingToolProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "combined-thinking-tool-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session_id)
                .await?;

            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Use the test tool"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);
            while let Some(event) = reply_stream.next().await {
                match event {
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            let reloaded = session_manager.get_session(&session_id, true).await?;
            let messages = reloaded
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec();

            assert_formatter_adds_reasoning_to_tool_calls(&messages, "combined-thinking-tool");
            Ok(())
        }

        /// Simulates the DeepSeek/Kimi multi-tool-call case: thinking arrives as a
        /// separate stream chunk, then both tool calls arrive together in a second
        /// chunk with no thinking.  Before the fix, the second tool-call message
        /// (asst(TC2)) received no reasoning_content because lines 210-213 in
        /// format_messages_with_options cleared tool_call_turn_reasoning after the
        /// first tool result.
        struct MultiToolThinkingProvider {
            call_count: AtomicUsize,
        }

        impl MultiToolThinkingProvider {
            fn new() -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for MultiToolThinkingProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "multi-tool-thinking-mock".to_string(),
                    display_name: "Multi Tool Thinking Mock".to_string(),
                    description: "Mock for multi-tool thinking preservation".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for MultiToolThinkingProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for MultiToolThinkingProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(20), Some(30)),
                );
                match call {
                    0 => {
                        // Chunk 1: reasoning only (no tool calls)
                        let thinking =
                            Message::assistant().with_thinking("multi-tool reasoning", "sig_0");
                        // Chunk 2: two tool calls, no reasoning — the multi-tool bug scenario
                        let tc1 = CallToolRequestParams::new("tool_a")
                            .with_arguments(object!({"p": "1"}));
                        let tc2 = CallToolRequestParams::new("tool_b")
                            .with_arguments(object!({"p": "2"}));
                        let tool_msg = Message::assistant()
                            .with_tool_request("call_1", Ok(tc1))
                            .with_tool_request("call_2", Ok(tc2));
                        let stream = futures::stream::iter(vec![
                            Ok((Some(thinking), None)),
                            Ok((Some(tool_msg), Some(usage))),
                        ]);
                        Ok(Box::pin(stream))
                    }
                    _ => {
                        let msg = Message::assistant().with_text("Done.");
                        Ok(Box::pin(futures::stream::once(async move {
                            Ok((Some(msg), Some(usage)))
                        })))
                    }
                }
            }

            fn get_name(&self) -> &str {
                "multi-tool-thinking-mock"
            }
        }

        #[tokio::test]
        async fn test_multi_tool_response_preserves_reasoning_and_message_id_correlation(
        ) -> Result<()> {
            use goose_providers::formats::openai::{
                format_messages_with_options, OpenAiFormatOptions,
            };
            use goose_providers::images::ImageFormat;

            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(MultiToolThinkingProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "multi-tool-thinking-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session_id)
                .await?;

            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Use both tools"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);
            let mut live_tool_message_id = None;
            let mut usage_message_ids = Vec::new();
            while let Some(event) = reply_stream.next().await {
                match event? {
                    AgentEvent::Message(message)
                        if message
                            .content
                            .iter()
                            .any(|content| matches!(content, MessageContent::ToolRequest(_))) =>
                    {
                        live_tool_message_id = message.id;
                    }
                    AgentEvent::MessageUsage { message_id, .. } => {
                        usage_message_ids.push(message_id);
                    }
                    _ => {}
                }
            }

            let reloaded = session_manager.get_session(&session_id, true).await?;
            let messages = reloaded
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec();

            let live_tool_message_id =
                live_tool_message_id.expect("live tool message must have a generated ID");
            let persisted_tool_message_ids: Vec<&str> = messages
                .iter()
                .filter(|message| {
                    message
                        .content
                        .iter()
                        .any(|content| matches!(content, MessageContent::ToolRequest(_)))
                })
                .map(|message| {
                    message
                        .id
                        .as_deref()
                        .expect("persisted tool message must have an ID")
                })
                .collect();

            assert_eq!(persisted_tool_message_ids.len(), 2);
            assert_ne!(
                persisted_tool_message_ids[0], persisted_tool_message_ids[1],
                "split tool messages must keep distinct message IDs"
            );
            assert_eq!(
                persisted_tool_message_ids
                    .iter()
                    .copied()
                    .filter(|message_id| *message_id == live_tool_message_id.as_str())
                    .count(),
                1,
                "exactly one persisted tool message must retain the live message ID"
            );
            assert!(
                usage_message_ids.iter().any(|message_id| {
                    message_id.as_deref() == Some(live_tool_message_id.as_str())
                }),
                "tool-turn usage must reference the live message ID"
            );

            let spec = format_messages_with_options(
                &messages,
                &ImageFormat::OpenAi,
                OpenAiFormatOptions {
                    preserve_thinking_context: true,
                    ..Default::default()
                },
            );

            // Both tool calls must end up in one merged assistant message with reasoning_content.
            let assistant_msgs: Vec<_> = spec
                .iter()
                .filter(|m| m.get("role") == Some(&serde_json::json!("assistant")))
                .filter(|m| {
                    m.get("tool_calls")
                        .and_then(|tc| tc.as_array())
                        .is_some_and(|a| !a.is_empty())
                })
                .collect();

            assert_eq!(
                assistant_msgs.len(),
                1,
                "both tool calls must be merged into one assistant message"
            );
            assert_eq!(
                assistant_msgs[0]["reasoning_content"], "multi-tool reasoning",
                "merged message must carry reasoning_content"
            );
            let tool_calls = assistant_msgs[0]["tool_calls"].as_array().unwrap();
            assert_eq!(tool_calls.len(), 2, "both tool calls must be present");

            Ok(())
        }

        /// Regression for the Anthropic 400: signed thinking arriving in a
        /// separate chunk before the tool calls must be stored once per
        /// tool-call message and never as an extra standalone message. When the
        /// Anthropic formatter serializes the persisted history, each assistant
        /// turn must carry exactly one thinking block — a duplicate signed block
        /// is rejected with `thinking blocks ... cannot be modified`.
        #[tokio::test]
        async fn test_signed_thinking_not_duplicated_for_anthropic() -> Result<()> {
            use goose_providers::formats::anthropic::format_messages as anthropic_format;

            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(MultiToolThinkingProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "anthropic-signed-thinking-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session_id)
                .await?;

            let session_config = SessionConfig {
                id: session_id.clone(),
                schedule_id: None,
                max_turns: Some(2),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Use both tools"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);
            while let Some(event) = reply_stream.next().await {
                event?;
            }

            let reloaded = session_manager.get_session(&session_id, true).await?;
            let messages = reloaded
                .conversation
                .expect("should have conversation")
                .messages()
                .to_vec();

            // No standalone thinking-only assistant message should be persisted —
            // thinking lives on the tool-call messages.
            let standalone_thinking = messages.iter().any(|m| {
                m.role == rmcp::model::Role::Assistant
                    && !m.content.is_empty()
                    && m.content
                        .iter()
                        .all(|c| matches!(c, MessageContent::Thinking(_)))
            });
            assert!(
                !standalone_thinking,
                "thinking must not be persisted as a standalone message: {messages:#?}"
            );

            // Every serialized Anthropic assistant message must contain at most
            // one thinking block; a duplicate is what triggers the 400.
            let spec = anthropic_format(&messages);
            for msg in &spec {
                if msg.get("role") == Some(&serde_json::json!("assistant")) {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                        let thinking_blocks = content
                            .iter()
                            .filter(|c| c.get("type") == Some(&serde_json::json!("thinking")))
                            .count();
                        assert!(
                            thinking_blocks <= 1,
                            "assistant message has {thinking_blocks} thinking blocks, \
                             Anthropic rejects duplicates: {msg}"
                        );
                    }
                }
            }

            Ok(())
        }
    }

    #[cfg(test)]
    mod goal_checking_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::AgentConfig;
        use goose::agents::SessionConfig;
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::Message;
        use goose::providers::base::{
            stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
        };
        use goose::session::session_manager::SessionType;
        use goose::session::SessionManager;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::Tool;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};
        use tempfile::TempDir;

        struct GoalTextProvider {
            call_count: AtomicU32,
        }

        impl GoalTextProvider {
            fn new() -> Self {
                Self {
                    call_count: AtomicU32::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for GoalTextProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "goal-mock".to_string(),
                    display_name: "Goal Mock Provider".to_string(),
                    description: "Mock provider for goal testing".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for GoalTextProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                Box::pin(async { Ok(Self::new()) })
            }
        }

        #[async_trait]
        impl Provider for GoalTextProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                let text = format!("Response number {count}");
                let message = Message::assistant().with_text(&text);
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(Some(10), Some(5), Some(15)),
                );
                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "goal-mock"
            }
        }

        fn create_agent_with_session_naming_disabled(
            session_manager: Arc<SessionManager>,
        ) -> Agent {
            let config = AgentConfig::new(
                session_manager,
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            Agent::with_config(config)
        }

        #[tokio::test]
        async fn test_goal_nudges_agent_before_exit() -> Result<()> {
            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let agent = create_agent_with_session_naming_disabled(session_manager.clone());
            let provider = Arc::new(GoalTextProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "goal-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;
            agent
                .set_goal(Some("Ensure the sky is blue".to_string()))
                .await;

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(10),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hello"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                match event {
                    Ok(AgentEvent::Message(msg)) => messages.push(msg),
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            let call_count = provider.call_count.load(Ordering::SeqCst);
            assert!(
                call_count > 1,
                "Expected provider to be called more than once due to goal checking, got {call_count}"
            );
            assert!(
                call_count <= 3,
                "Expected at most 3 provider calls (1 initial + 1 goal check + 1 exit), got {call_count}"
            );

            // The goal nudge should NOT appear in yielded events (it's internal)
            let nudge_messages: Vec<_> = messages
                .iter()
                .filter(|m| {
                    m.as_concat_text()
                        .contains("check whether the following goal")
                })
                .collect();
            assert!(
                nudge_messages.is_empty(),
                "Goal nudge should be hidden from user, but found {} in events",
                nudge_messages.len()
            );

            // Goal should be cleared after being met
            assert_eq!(
                agent.get_goal().await,
                None,
                "Goal should be cleared after the agent finishes with it met"
            );

            Ok(())
        }

        #[tokio::test]
        async fn test_no_goal_exits_immediately() -> Result<()> {
            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let agent = create_agent_with_session_naming_disabled(session_manager.clone());
            let provider = Arc::new(GoalTextProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "no-goal-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(10),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hello"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            while let Some(event) = reply_stream.next().await {
                match event {
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }

            let call_count = provider.call_count.load(Ordering::SeqCst);
            assert_eq!(
                call_count, 1,
                "Without a goal, provider should be called exactly once, got {call_count}"
            );

            Ok(())
        }

        #[tokio::test]
        async fn test_goal_command_set_and_clear() -> Result<()> {
            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let agent = create_agent_with_session_naming_disabled(session_manager.clone());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "goal-cmd-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            // No goal initially
            let result = agent.execute_command("/goal", &session.id).await?.unwrap();
            assert!(result.as_concat_text().contains("No goal set"));

            // Set a goal
            let result = agent
                .execute_command("/goal make all tests pass", &session.id)
                .await?
                .unwrap();
            assert!(result.as_concat_text().contains("Goal set"));
            assert_eq!(
                agent.get_goal().await,
                Some("make all tests pass".to_string())
            );

            // Query it
            let result = agent.execute_command("/goal", &session.id).await?.unwrap();
            assert!(result.as_concat_text().contains("make all tests pass"));

            // Clear it
            let result = agent
                .execute_command("/goal off", &session.id)
                .await?
                .unwrap();
            assert!(result.as_concat_text().contains("cleared"));
            assert_eq!(agent.get_goal().await, None);

            Ok(())
        }

        #[tokio::test]
        async fn test_setting_goal_via_reply_starts_a_turn() -> Result<()> {
            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let agent = create_agent_with_session_naming_disabled(session_manager.clone());
            let provider = Arc::new(GoalTextProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "goal-start-turn".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(10),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(
                    Message::user().with_text("/goal make all tests pass"),
                    session_config,
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let Ok(AgentEvent::Message(msg)) = event {
                    messages.push(msg);
                }
            }

            // The provider must be invoked: setting a goal kicks off a turn
            // (the goal-checking loop then runs and clears the goal once met).
            assert!(
                provider.call_count.load(Ordering::SeqCst) >= 1,
                "Setting a goal should start an agent turn"
            );

            // The user still sees the confirmation.
            assert!(
                messages
                    .iter()
                    .any(|m| m.as_concat_text().contains("Goal set")),
                "Goal confirmation should be surfaced to the user"
            );

            Ok(())
        }

        #[tokio::test]
        async fn test_querying_goal_via_reply_does_not_start_a_turn() -> Result<()> {
            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let agent = create_agent_with_session_naming_disabled(session_manager.clone());
            let provider = Arc::new(GoalTextProvider::new());

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "goal-query-no-turn".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(10),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("/goal"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut emitted_user_id = None;
            let mut emitted_response_id = None;
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(message) = event? {
                    if message.role == rmcp::model::Role::User
                        && message.as_concat_text() == "/goal"
                    {
                        emitted_user_id = message.id;
                    } else if message.role == rmcp::model::Role::Assistant
                        && message.as_concat_text().contains("No goal set")
                    {
                        emitted_response_id = message.id;
                    }
                }
            }

            assert_eq!(
                provider.call_count.load(Ordering::SeqCst),
                0,
                "Querying the goal should not start an agent turn"
            );

            let emitted_user_id = emitted_user_id.expect("User message should be emitted with ID");
            assert!(emitted_user_id.starts_with("msg_"));
            let emitted_response_id =
                emitted_response_id.expect("Slash command response should be emitted with ID");
            assert!(emitted_response_id.starts_with("msg_"));

            let reloaded = session_manager.get_session(&session.id, true).await?;
            let conversation = reloaded
                .conversation
                .expect("Session should have a conversation");
            let stored_user_message = conversation
                .messages()
                .iter()
                .find(|message| {
                    message.role == rmcp::model::Role::User && message.as_concat_text() == "/goal"
                })
                .expect("User message should be stored");

            assert_eq!(
                stored_user_message.id.as_deref(),
                Some(emitted_user_id.as_str())
            );
            let stored_response_message = conversation
                .messages()
                .iter()
                .find(|message| {
                    message.role == rmcp::model::Role::Assistant
                        && message.as_concat_text().contains("No goal set")
                })
                .expect("Slash command response should be stored");

            assert_eq!(
                stored_response_message.id.as_deref(),
                Some(emitted_response_id.as_str())
            );

            Ok(())
        }
    }

    mod cumulative_token_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::Message;
        use goose::providers::base::{stream_from_single_message, MessageStream, Provider};
        use goose::session::session_manager::SessionType;
        use goose::session::SessionManager;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::Tool;
        use std::path::PathBuf;
        use std::sync::Arc;

        struct FixedUsageProvider {
            input_tokens: i32,
            output_tokens: i32,
        }

        #[async_trait]
        impl Provider for FixedUsageProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let total = self.input_tokens + self.output_tokens;
                let usage = ProviderUsage::new(
                    "mock-model".to_string(),
                    Usage::new(
                        Some(self.input_tokens),
                        Some(self.output_tokens),
                        Some(total),
                    ),
                );
                let message = Message::assistant().with_text("Hello");
                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "fixed-usage-mock"
            }
        }

        async fn run_turn(agent: &Agent, session_id: &str, text: &str) -> Result<()> {
            let session_config = SessionConfig {
                id: session_id.to_string(),
                schedule_id: None,
                max_turns: Some(1),
                retry_config: None,
            };
            let stream = agent
                .reply(Message::user().with_text(text), session_config, None)
                .await?;
            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                let _ = event?;
            }
            Ok(())
        }

        #[tokio::test]
        async fn test_accumulated_total_tokens_across_multiple_turns() -> Result<()> {
            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let config = AgentConfig::new(
                session_manager.clone(),
                PermissionManager::instance(),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            );
            let agent = Agent::with_config(config);
            let provider = Arc::new(FixedUsageProvider {
                input_tokens: 10,
                output_tokens: 5,
            });

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "cumulative-token-test".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;

            let session_id = session.id.clone();
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session_id,
                )
                .await?;

            run_turn(&agent, &session_id, "Turn 1").await?;
            let after_1 = session_manager.get_session(&session_id, false).await?;
            assert_eq!(after_1.accumulated_usage.total_tokens, Some(15));

            run_turn(&agent, &session_id, "Turn 2").await?;
            let after_2 = session_manager.get_session(&session_id, false).await?;
            assert_eq!(after_2.accumulated_usage.total_tokens, Some(30));
            assert_eq!(after_2.usage.total_tokens, Some(15));

            Ok(())
        }
    }

    mod add_extensions_bulk_tests {
        use super::*;
        use goose::agents::extension::Envs;
        use goose::agents::{AgentConfig, ExtensionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::session::session_manager::SessionType;
        use goose::session::{
            EnabledExtensionsState, ExtensionData, ExtensionState, SessionManager,
        };
        use tempfile::TempDir;

        fn platform_extension(name: &str) -> ExtensionConfig {
            ExtensionConfig::Platform {
                name: name.to_string(),
                description: format!("Platform test extension {name}"),
                display_name: None,
                bundled: None,
                available_tools: vec![],
            }
        }

        fn unloadable_stdio_extension(name: &str) -> ExtensionConfig {
            ExtensionConfig::Stdio {
                name: name.to_string(),
                description: format!("Unloadable test extension {name}"),
                cmd: "goose-test-definitely-missing-binary".to_string(),
                args: vec![],
                envs: Envs::default(),
                env_keys: vec![],
                timeout: Some(1),
                cwd: None,
                bundled: None,
                available_tools: vec![],
            }
        }

        async fn setup_agent_and_session(
            test_name: &str,
        ) -> (Arc<Agent>, Arc<SessionManager>, String, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let data_dir = temp_dir.path().to_path_buf();
            let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
            let permission_manager = Arc::new(PermissionManager::new(data_dir));
            let agent = Arc::new(Agent::with_config(AgentConfig::new(
                session_manager.clone(),
                permission_manager,
                None,
                GooseMode::default(),
                false,
                GoosePlatform::GooseDesktop,
            )));

            let session = session_manager
                .create_session(
                    std::env::current_dir().unwrap(),
                    test_name.to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await
                .unwrap();

            (agent, session_manager, session.id, temp_dir)
        }

        async fn persisted_extension_names(
            session_manager: &SessionManager,
            session_id: &str,
        ) -> Vec<String> {
            let session = session_manager
                .get_session(session_id, false)
                .await
                .unwrap();
            let mut names: Vec<String> =
                EnabledExtensionsState::from_extension_data(&session.extension_data)
                    .expect("enabled extensions state should be persisted")
                    .extensions
                    .iter()
                    .map(|extension| extension.name())
                    .collect();
            names.sort();
            names
        }

        #[tokio::test]
        async fn test_bulk_load_persists_loaded_extensions() {
            let (agent, session_manager, session_id, _temp_dir) =
                setup_agent_and_session("bulk-load-persist-success").await;

            let results = agent
                .add_extensions_bulk(
                    vec![platform_extension("analyze"), platform_extension("todo")],
                    &session_id,
                )
                .await
                .unwrap();

            assert!(results.iter().all(|result| result.success));
            assert_eq!(
                persisted_extension_names(&session_manager, &session_id).await,
                vec!["analyze".to_string(), "todo".to_string()]
            );
        }

        #[tokio::test]
        async fn test_bulk_load_partial_failure_persists_only_loaded_extensions() {
            let (agent, session_manager, session_id, _temp_dir) =
                setup_agent_and_session("bulk-load-persist-partial-failure").await;

            let results = agent
                .add_extensions_bulk(
                    vec![
                        platform_extension("todo"),
                        unloadable_stdio_extension("broken"),
                    ],
                    &session_id,
                )
                .await
                .unwrap();

            assert_eq!(results.len(), 2);
            assert!(results
                .iter()
                .any(|result| result.name == "todo" && result.success));
            let broken = results
                .iter()
                .find(|result| result.name == "broken")
                .expect("broken extension should report a result");
            assert!(!broken.success);
            assert!(broken.error.is_some());

            assert_eq!(
                persisted_extension_names(&session_manager, &session_id).await,
                vec!["todo".to_string()]
            );
        }

        #[tokio::test]
        async fn test_bulk_load_total_failure_drops_failed_extensions_from_session_state() {
            let (agent, session_manager, session_id, _temp_dir) =
                setup_agent_and_session("bulk-load-persist-total-failure").await;

            // Seed the session with the extensions up front, mirroring a resume
            // where the enabled list is read back from session metadata.
            let extensions = vec![
                unloadable_stdio_extension("broken-one"),
                unloadable_stdio_extension("broken-two"),
            ];
            let mut extension_data = ExtensionData::new();
            EnabledExtensionsState::new(extensions.clone())
                .to_extension_data(&mut extension_data)
                .unwrap();
            session_manager
                .update(&session_id)
                .extension_data(extension_data)
                .apply()
                .await
                .unwrap();

            let results = agent
                .add_extensions_bulk(extensions, &session_id)
                .await
                .unwrap();

            assert_eq!(results.len(), 2);
            assert!(
                results.iter().all(|result| !result.success),
                "expected every extension load to fail: {results:?}"
            );

            // The failed extensions must not stay marked as enabled in the
            // session, otherwise every future resume retries them.
            assert_eq!(
                persisted_extension_names(&session_manager, &session_id).await,
                Vec::<String>::new()
            );
        }

        #[tokio::test]
        async fn test_bulk_load_cancellation_preserves_pending_extensions() {
            let (agent, session_manager, session_id, _temp_dir) =
                setup_agent_and_session("bulk-load-persist-cancellation").await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (_connection, _) = listener.accept().await.unwrap();
                let _ = accepted_tx.send(());
                std::future::pending::<()>().await;
            });

            let extension = ExtensionConfig::streamable_http(
                "pending".to_string(),
                format!("http://{address}"),
                "Pending test extension".to_string(),
                30_u64,
            );
            agent
                .persist_extension_configs(&session_id, vec![extension.clone()])
                .await
                .unwrap();
            let load = tokio::spawn({
                let agent = agent.clone();
                let session_id = session_id.clone();
                async move {
                    agent
                        .add_extensions_bulk(vec![extension], &session_id)
                        .await
                }
            });

            tokio::time::timeout(std::time::Duration::from_secs(5), accepted_rx)
                .await
                .expect("extension did not connect")
                .expect("test server stopped before accepting a connection");

            load.abort();
            assert!(load.await.unwrap_err().is_cancelled());
            server.abort();
            assert_eq!(
                persisted_extension_names(&session_manager, &session_id).await,
                vec!["pending".to_string()]
            );
        }
    }

    mod audience_tool_result_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::{AgentConfig, SessionConfig};
        use goose::config::{ExtensionConfig, GooseMode, PermissionManager};
        use goose::conversation::message::{Message, MessageContent};
        use goose::providers::base::{stream_from_single_message, MessageStream, Provider};
        use goose::session::{SessionManager, SessionType};
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use goose_test_support::McpFixture;
        use rmcp::model::{CallToolRequestParams, Tool};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct AudienceToolProvider {
            call_count: AtomicUsize,
        }

        fn tool_response_texts(messages: &[Message], id: &str) -> Option<Vec<String>> {
            messages.iter().find_map(|message| {
                message.content.iter().find_map(|content| {
                    let MessageContent::ToolResponse(response) = content else {
                        return None;
                    };
                    if response.id != id {
                        return None;
                    }
                    let result = response.tool_result.as_ref().ok()?;
                    Some(
                        result
                            .content
                            .iter()
                            .filter_map(|content| content.as_text().map(|text| text.text.clone()))
                            .collect(),
                    )
                })
            })
        }

        #[async_trait]
        impl Provider for AudienceToolProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                let message = match call {
                    0 => Message::assistant().with_tool_request(
                        "call-1",
                        Ok(CallToolRequestParams::new(
                            "mcp-fixture__get_audience_content",
                        )),
                    ),
                    1 => {
                        assert_eq!(
                            tool_response_texts(messages, "call-1"),
                            Some(vec!["visible".to_string(), "provider-only".to_string()]),
                            "provider history must retain canonical tool content"
                        );
                        Message::assistant().with_text("done")
                    }
                    _ => panic!("unexpected provider call {call}"),
                };
                let usage = ProviderUsage::new("mock-model".to_string(), Usage::default());
                Ok(stream_from_single_message(message, usage))
            }

            fn get_name(&self) -> &str {
                "audience-tool-mock"
            }
        }

        #[tokio::test]
        async fn live_tool_result_projects_user_content_but_persists_canonical_result() -> Result<()>
        {
            let mcp = McpFixture::new().await;
            let extension =
                ExtensionConfig::streamable_http("mcp-fixture", &mcp.url, "MCP fixture", 30_u64);
            let temp_dir = tempfile::tempdir()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
            let permission_manager =
                Arc::new(PermissionManager::new(temp_dir.path().to_path_buf()));
            let agent = Agent::with_config(AgentConfig::new(
                session_manager.clone(),
                permission_manager,
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            ));
            let provider = Arc::new(AudienceToolProvider {
                call_count: AtomicUsize::new(0),
            });
            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "audience-tool-result".to_string(),
                    SessionType::Hidden,
                    GooseMode::Auto,
                )
                .await?;
            let session_id = session.id.clone();
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session_id,
                )
                .await?;
            agent.add_extension(extension, &session_id).await?;

            let stream = agent
                .reply(
                    Message::user().with_text("use the audience tool"),
                    SessionConfig {
                        id: session_id.clone(),
                        schedule_id: None,
                        max_turns: Some(3),
                        retry_config: None,
                    },
                    None,
                )
                .await?;
            tokio::pin!(stream);
            let mut live_messages = Vec::new();
            while let Some(event) = stream.next().await {
                if let AgentEvent::Message(message) = event? {
                    live_messages.push(message);
                }
            }

            assert_eq!(
                tool_response_texts(&live_messages, "call-1"),
                Some(vec!["visible".to_string()]),
                "live events must project out provider-only tool content"
            );
            assert_eq!(provider.call_count.load(Ordering::SeqCst), 2);

            let persisted = session_manager
                .get_session(&session_id, true)
                .await?
                .conversation
                .expect("persisted conversation");
            assert_eq!(
                tool_response_texts(persisted.messages(), "call-1"),
                Some(vec!["visible".to_string(), "provider-only".to_string()]),
                "persisted provider history must remain canonical"
            );
            Ok(())
        }
    }

    mod empty_turn_tests {
        use super::*;
        use async_trait::async_trait;
        use goose::agents::final_output_tool::FINAL_OUTPUT_TOOL_NAME;
        use goose::agents::{AgentConfig, AgentEvent, GoosePlatform, SessionConfig};
        use goose::config::permission::PermissionManager;
        use goose::config::GooseMode;
        use goose::conversation::message::{Message, MessageContent};
        use goose::conversation::Conversation;
        use goose::providers::base::{
            stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
        };
        use goose::session::session_manager::SessionType;
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;
        use rmcp::model::{CallToolRequestParams, Tool};
        use rmcp::object;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        fn usage() -> ProviderUsage {
            ProviderUsage::new(
                "mock-model".to_string(),
                Usage::new(Some(10), Some(5), Some(15)),
            )
        }

        /// Yields empty responses (no text, no tool calls) for the first
        /// `empty_count` provider calls, then a normal text response.
        struct EmptyThenTextProvider {
            call_count: AtomicUsize,
            empty_count: usize,
            wrap_empty_text: bool,
            manages_own_context: bool,
        }

        struct AssistantOnlyProvider;

        struct FinalOutputRequestProvider {
            call_count: AtomicUsize,
        }

        impl goose::providers::base::ProviderDescriptor for AssistantOnlyProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "assistant-only-mock".to_string(),
                    display_name: "Assistant Only Mock".to_string(),
                    description: "Mock provider for audience-filtered response tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for AssistantOnlyProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for AssistantOnlyProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                use rmcp::model::{Annotations, Role, TextContent};

                let assistant_only = TextContent::new("provider-private-state")
                    .with_annotations(Annotations::default().with_audience(vec![Role::Assistant]));
                Ok(stream_from_single_message(
                    Message::assistant().with_content(MessageContent::Text(assistant_only)),
                    usage(),
                ))
            }

            fn get_name(&self) -> &str {
                "assistant-only-mock"
            }
        }

        impl EmptyThenTextProvider {
            fn new(empty_count: usize) -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                    empty_count,
                    wrap_empty_text: false,
                    manages_own_context: false,
                }
            }

            fn with_wrapped_empty_text(empty_count: usize) -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                    empty_count,
                    wrap_empty_text: true,
                    manages_own_context: false,
                }
            }

            fn with_own_context() -> Self {
                Self {
                    manages_own_context: true,
                    ..Self::new(usize::MAX)
                }
            }
        }

        impl FinalOutputRequestProvider {
            fn new() -> Self {
                Self {
                    call_count: AtomicUsize::new(0),
                }
            }
        }

        impl goose::providers::base::ProviderDescriptor for EmptyThenTextProvider {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata {
                    name: "empty-then-text-mock".to_string(),
                    display_name: "Empty Then Text Mock".to_string(),
                    description: "Mock provider for empty-turn tests".to_string(),
                    default_model: "mock-model".to_string(),
                    known_models: vec![],
                    model_doc_link: "".to_string(),
                    config_keys: vec![],
                    setup_steps: vec![],
                    setup: None,
                    deprecated: None,
                }
            }
        }

        impl ProviderDef for EmptyThenTextProvider {
            type Provider = Self;

            fn from_env(
                _extensions: Vec<goose::config::ExtensionConfig>,
                _tls_config: Option<goose::providers::api_client::TlsConfig>,
            ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
                unimplemented!()
            }
        }

        #[async_trait]
        impl Provider for EmptyThenTextProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                if call < self.empty_count {
                    // Empty assistant turn: no text, no tool calls.
                    let message = if self.wrap_empty_text {
                        Message::assistant().with_text("")
                    } else {
                        Message::assistant()
                    };
                    Ok(stream_from_single_message(message, usage()))
                } else {
                    Ok(stream_from_single_message(
                        Message::assistant().with_text("All done."),
                        usage(),
                    ))
                }
            }

            fn get_name(&self) -> &str {
                "empty-then-text-mock"
            }

            fn manages_own_context(&self) -> bool {
                self.manages_own_context
            }
        }

        #[async_trait]
        impl Provider for FinalOutputRequestProvider {
            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system_prompt: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> Result<MessageStream, ProviderError> {
                let call = self.call_count.fetch_add(1, Ordering::SeqCst);
                if call != 0 {
                    panic!("unexpected provider call after final-output tool request");
                }

                let tool_call = CallToolRequestParams::new(FINAL_OUTPUT_TOOL_NAME)
                    .with_arguments(object!({"result": "Final answer"}));
                Ok(stream_from_single_message(
                    Message::assistant().with_tool_request("final-output-call", Ok(tool_call)),
                    usage(),
                ))
            }

            fn get_name(&self) -> &str {
                "final-output-request-mock"
            }
        }

        /// Runs a reply to completion and returns the messages yielded to the
        /// caller along with the conversation persisted to the session store.
        async fn run_reply(
            provider: Arc<dyn Provider>,
            session_name: &str,
        ) -> Result<(Vec<Message>, Vec<Message>)> {
            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    session_name.to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
                .await?;

            let session_id = session.id.clone();
            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(50),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(m) = event? {
                    messages.push(m);
                }
            }

            let persisted = agent
                .config
                .session_manager
                .get_session(&session_id, true)
                .await?
                .conversation
                .map(|c| c.messages().to_vec())
                .unwrap_or_default();

            Ok((messages, persisted))
        }

        fn concat_text(messages: &[Message]) -> String {
            messages
                .iter()
                .flat_map(|m| m.content.iter())
                .filter_map(|c| match c {
                    MessageContent::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        fn is_empty_assistant(message: &Message) -> bool {
            message.role == rmcp::model::Role::Assistant && message.content.is_empty()
        }

        /// A transient empty response should be retried and recover, ultimately
        /// delivering the real text response instead of stopping silently.
        #[tokio::test]
        async fn test_empty_turn_retries_then_recovers() -> Result<()> {
            let provider = Arc::new(EmptyThenTextProvider::new(2));
            let (messages, persisted) = run_reply(provider, "empty-retry-recover").await?;

            let text = concat_text(&messages);
            assert!(
                text.contains("All done."),
                "expected recovery to deliver the real response, got: {text:?}"
            );
            assert!(
                !text.contains("empty response"),
                "should not surface the empty-turn fallback when recovery succeeds: {text:?}"
            );
            assert!(
                !persisted.iter().any(is_empty_assistant),
                "retried empty turns must not be persisted: {persisted:?}"
            );
            Ok(())
        }

        #[tokio::test]
        async fn test_wrapped_empty_text_retries_then_recovers() -> Result<()> {
            let provider = Arc::new(EmptyThenTextProvider::with_wrapped_empty_text(1));
            let (messages, persisted) = run_reply(provider, "wrapped-empty-retry").await?;

            assert!(concat_text(&messages).contains("All done."));
            assert!(!persisted.iter().any(|message| {
                message.role == rmcp::model::Role::Assistant
                    && matches!(message.content.as_slice(), [MessageContent::Text(text)] if text.text.is_empty())
            }));
            Ok(())
        }

        /// A provider that only ever returns empty responses must not hang
        /// silently — after the retry budget it surfaces a visible message.
        #[tokio::test]
        async fn test_persistent_empty_turn_surfaces_message() -> Result<()> {
            let provider = Arc::new(EmptyThenTextProvider::new(usize::MAX));
            let (messages, persisted) = run_reply(provider, "empty-persistent").await?;

            let text = concat_text(&messages);
            assert!(
                text.contains("empty response"),
                "expected a visible empty-response message, got: {text:?}"
            );

            let last = messages.last().expect("expected at least one message");
            assert!(
                matches!(last.content.first(), Some(MessageContent::Text(_))),
                "expected the final message to be visible text, got: {:?}",
                last.content
            );
            assert!(
                !persisted.iter().any(is_empty_assistant),
                "empty assistant turn must not be persisted alongside the fallback: {persisted:?}"
            );

            let emitted_fallback_id = last
                .id
                .as_deref()
                .expect("empty-turn fallback should be emitted with ID");
            assert!(emitted_fallback_id.starts_with("msg_"));

            let stored_fallback = persisted
                .iter()
                .find(|message| message.as_concat_text().contains("empty response"))
                .expect("empty-turn fallback should be stored");
            assert_eq!(stored_fallback.id.as_deref(), Some(emitted_fallback_id));
            Ok(())
        }

        #[tokio::test]
        async fn test_assistant_only_response_is_persisted_without_empty_turn_retry() -> Result<()>
        {
            let provider = Arc::new(AssistantOnlyProvider);
            let (messages, persisted) = run_reply(provider, "assistant-only-response").await?;

            assert!(
                messages.iter().all(|message| !is_empty_assistant(message)),
                "audience filtering must not emit an empty user-visible message: {messages:?}"
            );
            assert!(
                messages
                    .iter()
                    .all(|message| !message.as_concat_text().contains("provider-private-state")),
                "assistant-only content must not be emitted to the user: {messages:?}"
            );
            assert!(
                !concat_text(&messages).contains("empty response"),
                "assistant-only content must not trigger the empty-turn fallback: {messages:?}"
            );
            assert!(persisted.iter().any(|message| {
                message.role == rmcp::model::Role::Assistant
                    && message.as_concat_text() == "provider-private-state"
            }));
            let restored = Conversation::new_unvalidated(persisted.clone()).user_visible_messages();
            assert!(
                !concat_text(&restored).contains("provider-private-state"),
                "restored user history must project out assistant-only content: {restored:?}"
            );
            Ok(())
        }

        /// An empty response with a queued steer hands the turn to the steer
        /// rather than the empty-turn fallback, but the empty assistant message
        /// must still not be persisted ahead of the steer.
        #[tokio::test]
        async fn test_empty_response_with_steer_drops_empty_message() -> Result<()> {
            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "empty-steer".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(
                    Arc::new(EmptyThenTextProvider::new(1)),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            // Queue the steer before reply so it stays pending through the first
            // (empty) turn instead of being drained at the loop's start.
            agent
                .steer(&session.id, Message::user().with_text("keep going"))
                .await;

            let session_id = session.id.clone();
            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(50),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);
            let mut emitted_steer_id = None;
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(message) = event? {
                    if message.role == rmcp::model::Role::User
                        && message.as_concat_text().contains("keep going")
                    {
                        emitted_steer_id = message.id;
                    }
                }
            }

            let persisted = agent
                .config
                .session_manager
                .get_session(&session_id, true)
                .await?
                .conversation
                .map(|c| c.messages().to_vec())
                .unwrap_or_default();

            assert!(
                !persisted.iter().any(is_empty_assistant),
                "empty assistant turn must not be persisted before the steer: {persisted:?}"
            );
            assert!(
                persisted
                    .iter()
                    .any(|m| m.as_concat_text().contains("keep going")),
                "the queued steer should have been consumed: {persisted:?}"
            );
            let emitted_steer_id =
                emitted_steer_id.expect("queued steer should be emitted with ID");
            assert!(emitted_steer_id.starts_with("msg_"));
            let stored_steer = persisted
                .iter()
                .find(|message| message.as_concat_text().contains("keep going"))
                .expect("queued steer should be stored");
            assert_eq!(stored_steer.id.as_deref(), Some(emitted_steer_id.as_str()));
            Ok(())
        }

        #[tokio::test]
        async fn legacy_structured_output_fails_before_provider_inference() -> Result<()> {
            use goose::recipe::Response;

            let _guard = env_lock::lock_env([("GOOSE_STATE_MACHINE", None::<&str>)]);
            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "unsupported-structured-output".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            let provider = Arc::new(EmptyThenTextProvider::with_own_context());
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;
            agent
                .add_final_output_tool(Response {
                    json_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": { "result": { "type": "string" } }
                    })),
                })
                .await?;

            let reply_stream = agent
                .reply(
                    Message::user().with_text("Hi"),
                    SessionConfig {
                        id: session.id,
                        schedule_id: None,
                        max_turns: Some(3),
                        retry_config: None,
                    },
                    None,
                )
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(message) = event? {
                    messages.push(message);
                }
            }

            let text = concat_text(&messages);
            assert!(
                text.contains("empty-then-text-mock") && text.contains("final_output"),
                "expected the unsupported structured-output error, got: {text:?}"
            );
            assert_eq!(provider.call_count.load(Ordering::SeqCst), 0);

            Ok(())
        }

        /// When a final-output tool is installed and the model stops without
        /// calling it, the empty turn must yield the mandatory final-output nudge
        /// — not the generic empty-response fallback — so structured-output
        /// recipes are not abandoned without producing a result.
        #[tokio::test]
        async fn test_empty_turn_with_final_output_tool_nudges() -> Result<()> {
            use goose::agents::final_output_tool::FINAL_OUTPUT_CONTINUATION_MESSAGE;
            use goose::recipe::Response;

            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "empty-final-output".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(
                    Arc::new(EmptyThenTextProvider::new(usize::MAX)),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;
            agent
                .add_final_output_tool(Response {
                    json_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": { "result": { "type": "string" } }
                    })),
                })
                .await?;

            let session_config = SessionConfig {
                id: session.id.clone(),
                schedule_id: None,
                max_turns: Some(3),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            let mut emitted_nudge_ids = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(m) = event? {
                    if m.role == rmcp::model::Role::User
                        && m.as_concat_text()
                            .contains(FINAL_OUTPUT_CONTINUATION_MESSAGE)
                    {
                        emitted_nudge_ids.push(
                            m.id.clone()
                                .expect("Final-output nudge should be emitted with ID"),
                        );
                    }
                    messages.push(m);
                }
            }

            let text = concat_text(&messages);
            assert!(
                text.contains(FINAL_OUTPUT_CONTINUATION_MESSAGE),
                "expected the final-output nudge, got: {text:?}"
            );
            assert!(
                !text.contains("empty response"),
                "empty-turn fallback must not pre-empt the final-output nudge: {text:?}"
            );

            assert!(
                !emitted_nudge_ids.is_empty(),
                "expected at least one emitted final-output nudge"
            );
            assert!(emitted_nudge_ids.iter().all(|id| id.starts_with("msg_")));

            let reloaded = agent
                .config
                .session_manager
                .get_session(&session.id, true)
                .await?;
            let conversation = reloaded
                .conversation
                .expect("Session should have a conversation");
            let stored_nudge_ids = conversation
                .messages()
                .iter()
                .filter(|message| {
                    message.role == rmcp::model::Role::User
                        && message
                            .as_concat_text()
                            .contains(FINAL_OUTPUT_CONTINUATION_MESSAGE)
                })
                .map(|message| {
                    message
                        .id
                        .clone()
                        .expect("Stored final-output nudge should have ID")
                })
                .collect::<Vec<_>>();

            assert_eq!(stored_nudge_ids, emitted_nudge_ids);
            Ok(())
        }

        #[tokio::test]
        async fn test_final_output_result_id_matches_persisted_message() -> Result<()> {
            use goose::recipe::Response;
            use goose::session::SessionManager;
            use tempfile::TempDir;

            let temp_dir = TempDir::new()?;
            let session_manager = Arc::new(SessionManager::new(temp_dir.path().join("data")));
            let agent = Agent::with_config(AgentConfig::new(
                session_manager.clone(),
                Arc::new(PermissionManager::new(temp_dir.path().join("config"))),
                None,
                GooseMode::Auto,
                true,
                GoosePlatform::GooseCli,
            ));

            let session = session_manager
                .create_session(
                    PathBuf::default(),
                    "final-output-result".to_string(),
                    SessionType::Hidden,
                    GooseMode::Auto,
                )
                .await?;
            let session_id = session.id.clone();
            let provider = Arc::new(FinalOutputRequestProvider::new());
            agent
                .update_provider(
                    provider.clone(),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;
            agent
                .add_final_output_tool(Response {
                    json_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": { "result": { "type": "string" } },
                        "required": ["result"]
                    })),
                })
                .await?;

            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(5),
                retry_config: None,
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(m) = event? {
                    messages.push(m);
                }
            }

            let emitted_final_output = messages
                .iter()
                .find(|message| {
                    message.role == rmcp::model::Role::Assistant
                        && message.as_concat_text().contains("Final answer")
                })
                .expect("final-output result should be emitted");
            let emitted_final_output_id = emitted_final_output
                .id
                .as_deref()
                .expect("final-output result should be emitted with ID");
            assert!(emitted_final_output_id.starts_with("msg_"));

            let persisted = session_manager
                .get_session(&session_id, true)
                .await?
                .conversation
                .map(|c| c.messages().to_vec())
                .unwrap_or_default();
            let stored_final_output = persisted
                .iter()
                .find(|message| {
                    message.role == rmcp::model::Role::Assistant
                        && message.as_concat_text().contains("Final answer")
                })
                .expect("final-output result should be stored");

            assert_eq!(
                stored_final_output.id.as_deref(),
                Some(emitted_final_output_id)
            );
            assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);
            Ok(())
        }

        /// A recipe with retry_config owns the turn: recipe retry logic runs
        /// its success checks before the empty-turn fallback. When the check
        /// already passes, an empty final turn is the successful end of the
        /// recipe, not a generic empty-response error.
        #[tokio::test]
        async fn test_empty_turn_defers_to_recipe_retry() -> Result<()> {
            use goose::agents::types::{RetryConfig, SuccessCheck};

            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "empty-recipe-retry".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            agent
                .update_provider(
                    Arc::new(EmptyThenTextProvider::new(usize::MAX)),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(3),
                retry_config: Some(RetryConfig {
                    max_retries: 2,
                    checks: vec![SuccessCheck::Shell {
                        command: "true".to_string(),
                    }],
                    on_failure: None,
                    timeout_seconds: Some(30),
                    on_failure_timeout_seconds: None,
                }),
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(m) = event? {
                    messages.push(m);
                }
            }

            let text = concat_text(&messages);
            assert!(
                !text.contains("empty response"),
                "recipe retry (passing check) must own the empty turn, not the fallback: {text:?}"
            );
            Ok(())
        }

        /// When a recipe exhausts its retries on empty turns, the max-attempts
        /// failure message must be surfaced and persisted — not swallowed into a
        /// silent stop.
        #[tokio::test]
        async fn test_recipe_max_retries_surfaces_failure() -> Result<()> {
            use goose::agents::types::{RetryConfig, SuccessCheck};

            let agent = Agent::new();
            let session = agent
                .config
                .session_manager
                .create_session(
                    PathBuf::default(),
                    "recipe-max-retries".to_string(),
                    SessionType::Hidden,
                    GooseMode::default(),
                )
                .await?;
            let session_id = session.id.clone();
            agent
                .update_provider(
                    Arc::new(EmptyThenTextProvider::new(usize::MAX)),
                    ModelConfig::new("mock-model"),
                    &session.id,
                )
                .await?;

            let session_config = SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(5),
                retry_config: Some(RetryConfig {
                    max_retries: 1,
                    checks: vec![SuccessCheck::Shell {
                        command: "false".to_string(),
                    }],
                    on_failure: None,
                    timeout_seconds: Some(30),
                    on_failure_timeout_seconds: None,
                }),
            };

            let reply_stream = agent
                .reply(Message::user().with_text("Hi"), session_config, None)
                .await?;
            tokio::pin!(reply_stream);

            let mut messages = Vec::new();
            while let Some(event) = reply_stream.next().await {
                if let AgentEvent::Message(m) = event? {
                    messages.push(m);
                }
            }

            let text = concat_text(&messages);
            assert!(
                text.contains("Maximum retry attempts"),
                "exhausted recipe retries must surface the failure message: {text:?}"
            );
            let emitted_failure = messages
                .iter()
                .find(|message| message.as_concat_text().contains("Maximum retry attempts"))
                .expect("max-retry failure message should be emitted");
            let emitted_failure_id = emitted_failure
                .id
                .as_deref()
                .expect("max-retry failure message should be emitted with ID");
            assert!(emitted_failure_id.starts_with("msg_"));

            let persisted = agent
                .config
                .session_manager
                .get_session(&session_id, true)
                .await?
                .conversation
                .map(|c| c.messages().to_vec())
                .unwrap_or_default();
            assert!(
                concat_text(&persisted).contains("Maximum retry attempts"),
                "the max-retry failure message must be persisted: {persisted:?}"
            );
            let stored_failure = persisted
                .iter()
                .find(|message| message.as_concat_text().contains("Maximum retry attempts"))
                .expect("max-retry failure message should be stored");
            assert_eq!(stored_failure.id.as_deref(), Some(emitted_failure_id));
            Ok(())
        }
    }
}
