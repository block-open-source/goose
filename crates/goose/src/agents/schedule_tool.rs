//! Schedule management tool business logic.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::mcp_utils::ToolResult;
use chrono::Utc;
use rmcp::model::{Annotations, ContentBlock, ErrorCode, ErrorData, Role, TextContent};

use crate::conversation::Conversation;
use crate::recipe::template_recipe::parse_recipe_content;
use crate::recipe::validate_recipe::validate_recipe_template_from_content;
use crate::scheduler::{
    open_regular_schedule_recipe, ValidatedScheduleRecipe, MAX_SCHEDULE_RECIPE_BYTES,
};
use crate::scheduler_trait::SchedulerTrait;
use crate::session::SessionManager;

fn recipe_file_error(message: &str) -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, message.to_string(), None)
}

fn read_schedule_recipe(path: &Path) -> Result<(String, PathBuf), ErrorData> {
    let canonical_path = path
        .canonicalize()
        .map_err(|_| recipe_file_error("Cannot read recipe file"))?;
    let file = open_regular_schedule_recipe(&canonical_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidInput {
            recipe_file_error("Recipe path must reference a regular file")
        } else {
            recipe_file_error("Cannot read recipe file")
        }
    })?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| recipe_file_error("Cannot read recipe file"))?;
    if !opened_metadata.is_file() {
        return Err(recipe_file_error(
            "Recipe path must reference a regular file",
        ));
    }
    if opened_metadata.len() > MAX_SCHEDULE_RECIPE_BYTES {
        return Err(recipe_file_error(
            "Recipe file exceeds the 1048576 byte limit",
        ));
    }

    let mut bytes = Vec::new();
    file.take(MAX_SCHEDULE_RECIPE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recipe_file_error("Cannot read recipe file"))?;
    if bytes.len() as u64 > MAX_SCHEDULE_RECIPE_BYTES {
        return Err(recipe_file_error(
            "Recipe file exceeds the 1048576 byte limit",
        ));
    }

    let content = String::from_utf8(bytes)
        .map_err(|_| recipe_file_error("Recipe file must be valid UTF-8"))?;
    Ok((content, canonical_path))
}

pub struct ScheduleTool {
    scheduler: Arc<dyn SchedulerTrait>,
    session_manager: Arc<SessionManager>,
}

impl ScheduleTool {
    pub fn new(scheduler: Arc<dyn SchedulerTrait>, session_manager: Arc<SessionManager>) -> Self {
        Self {
            scheduler,
            session_manager,
        }
    }

    pub async fn execute(&self, arguments: serde_json::Value) -> ToolResult<Vec<ContentBlock>> {
        let action = arguments
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "Missing 'action' parameter".to_string(),
                    None,
                )
            })?;

        match action {
            "list" => self.handle_list_jobs(self.scheduler.clone()).await,
            "create" => {
                self.handle_create_job(self.scheduler.clone(), arguments)
                    .await
            }
            "run_now" => self.handle_run_now(self.scheduler.clone(), arguments).await,
            "pause" => {
                self.handle_pause_job(self.scheduler.clone(), arguments)
                    .await
            }
            "unpause" => {
                self.handle_unpause_job(self.scheduler.clone(), arguments)
                    .await
            }
            "delete" => {
                self.handle_delete_job(self.scheduler.clone(), arguments)
                    .await
            }
            "kill" => {
                self.handle_kill_job(self.scheduler.clone(), arguments)
                    .await
            }
            "inspect" => {
                self.handle_inspect_job(self.scheduler.clone(), arguments)
                    .await
            }
            "sessions" => {
                self.handle_list_sessions(self.scheduler.clone(), arguments)
                    .await
            }
            "session_content" => self.handle_session_content(arguments).await,
            _ => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Unknown action: {}", action),
                None,
            )),
        }
    }

    async fn handle_list_jobs(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
    ) -> ToolResult<Vec<ContentBlock>> {
        let jobs = scheduler.list_scheduled_jobs().await;
        let jobs_json = serde_json::to_string_pretty(&jobs).map_err(|e| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to serialize jobs: {}", e),
                None,
            )
        })?;
        Ok(vec![ContentBlock::text(format!(
            "Scheduled Jobs:\n{}",
            jobs_json
        ))])
    }

    async fn handle_create_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let recipe_path = arguments
            .get("recipe_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "Missing 'recipe_path' parameter".to_string(),
                    None,
                )
            })?;

        let cron_expression = arguments
            .get("cron_expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "Missing 'cron_expression' parameter".to_string(),
                    None,
                )
            })?;

        // Get the execution_mode parameter, defaulting to "background" if not provided
        let execution_mode = arguments
            .get("execution_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("background");

        let (content, canonical_recipe_path) = read_schedule_recipe(Path::new(recipe_path))?;
        let recipe_dir = canonical_recipe_path
            .parent()
            .map(|path| path.to_string_lossy().into_owned());
        // Parse failures echo the file back in the serde error, so they only ever
        // get the generic message; the checks that follow describe the recipe's
        // own shape and are safe to report.
        parse_recipe_content(&content, recipe_dir.clone()).map_err(|_| {
            if recipe_path.ends_with(".json") {
                recipe_file_error("Invalid JSON recipe")
            } else {
                recipe_file_error("Invalid YAML recipe")
            }
        })?;
        validate_recipe_template_from_content(&content, recipe_dir)
            .map_err(|error| recipe_file_error(&error.to_string()))?;

        // Generate unique job ID
        let job_id = format!("agent_created_{}", uuid::Uuid::new_v4());

        let recipe_base_dir = canonical_recipe_path
            .parent()
            .map(|path| path.to_string_lossy().into_owned());
        let job = crate::scheduler::ScheduledJob {
            id: job_id.clone(),
            source: canonical_recipe_path.to_string_lossy().into_owned(),
            cron: cron_expression.to_string(),
            last_run: None,
            currently_running: false,
            paused: false,
            current_session_id: None,
            process_start_time: None,
            parameters: vec![],
            recipe_base_dir,
        };

        match scheduler
            .add_scheduled_job_with_recipe(
                job,
                ValidatedScheduleRecipe::new(content.into_bytes(), canonical_recipe_path),
            )
            .await
        {
            Ok(()) => Ok(vec![ContentBlock::text(format!(
                "Successfully created scheduled job '{}' for recipe '{}' with cron expression '{}' in {} mode",
                job_id, recipe_path, cron_expression, execution_mode
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to create job: {}", e),
                None,
            )),
        }
    }

    /// Run a scheduled job immediately
    async fn handle_run_now(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.run_now(job_id).await {
            Ok(session_id) => Ok(vec![ContentBlock::text(format!(
                "Successfully started job '{}'. Session ID: {}",
                job_id, session_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to run job: {}", e),
                None,
            )),
        }
    }

    /// Pause a scheduled job
    async fn handle_pause_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.pause_schedule(job_id).await {
            Ok(()) => Ok(vec![ContentBlock::text(format!(
                "Successfully paused job '{}'",
                job_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to pause job: {}", e),
                None,
            )),
        }
    }

    /// Resume a paused scheduled job
    async fn handle_unpause_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.unpause_schedule(job_id).await {
            Ok(()) => Ok(vec![ContentBlock::text(format!(
                "Successfully unpaused job '{}'",
                job_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to unpause job: {}", e),
                None,
            )),
        }
    }

    /// Delete a scheduled job
    async fn handle_delete_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.remove_scheduled_job(job_id, false).await {
            Ok(()) => Ok(vec![ContentBlock::text(format!(
                "Successfully removed schedule for job '{}'",
                job_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to remove schedule: {}", e),
                None,
            )),
        }
    }

    /// Terminate a currently running job
    async fn handle_kill_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.kill_running_job(job_id).await {
            Ok(()) => Ok(vec![ContentBlock::text(format!(
                "Successfully killed running job '{}'",
                job_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to kill job: {}", e),
                None,
            )),
        }
    }

    /// Get information about a running job
    async fn handle_inspect_job(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        match scheduler.get_running_job_info(job_id).await {
            Ok(Some((session_id, start_time))) => {
                let duration = Utc::now().signed_duration_since(start_time);
                Ok(vec![ContentBlock::text(format!(
                    "Job '{}' is currently running:\n- Session ID: {}\n- Started: {}\n- Duration: {} seconds",
                    job_id,
                    session_id,
                    start_time.to_rfc3339(),
                    duration.num_seconds()
                ))])
            }
            Ok(None) => Ok(vec![ContentBlock::text(format!(
                "Job '{}' is not currently running",
                job_id
            ))]),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to inspect job: {}", e),
                None,
            )),
        }
    }

    /// List execution sessions for a job
    async fn handle_list_sessions(
        &self,
        scheduler: Arc<dyn SchedulerTrait>,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let job_id = arguments
            .get("job_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    "Missing 'job_id' parameter".to_string(),
                    None,
                )
            })?;

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as usize;

        match scheduler.sessions(job_id, limit).await {
            Ok(sessions) => {
                if sessions.is_empty() {
                    Ok(vec![ContentBlock::text(format!(
                        "No sessions found for job '{}'",
                        job_id
                    ))])
                } else {
                    let sessions_info: Vec<String> = sessions
                        .into_iter()
                        .map(|(session_name, session)| {
                            format!(
                                "- Session: {} (Messages: {}, Working Dir: {})",
                                session_name,
                                session.message_count,
                                session.working_dir.display()
                            )
                        })
                        .collect();

                    Ok(vec![ContentBlock::text(format!(
                        "Sessions for job '{}':\n{}",
                        job_id,
                        sessions_info.join("\n")
                    ))])
                }
            }
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list sessions: {}", e),
                None,
            )),
        }
    }

    /// Get the full content (metadata and messages) of a specific session
    async fn handle_session_content(
        &self,
        arguments: serde_json::Value,
    ) -> ToolResult<Vec<ContentBlock>> {
        let session_id = arguments
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Missing 'session_id' parameter".to_string(),
                    None,
                )
            })?;

        let mut session = match self.session_manager.get_session(session_id, true).await {
            Ok(metadata) => metadata,
            Err(e) => {
                return Err(ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to read session for '{}': {}", session_id, e),
                    None,
                ));
            }
        };

        session.conversation = session.conversation.map(|conversation| {
            Conversation::new_unvalidated(conversation.agent_visible_messages())
        });

        // Format the response with metadata and messages
        let metadata_json = match serde_json::to_string_pretty(&session) {
            Ok(json) => json,
            Err(e) => {
                return Err(ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to serialize metadata: {}", e),
                    None,
                ));
            }
        };

        Ok(vec![ContentBlock::Text(
            TextContent::new(format!(
                "Session '{}' Content:\n\nSession:\n{}",
                session_id, metadata_json
            ))
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
        )])
    }
}
