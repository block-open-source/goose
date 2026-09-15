use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::tool_execution::ToolCallContext;
use crate::agents::{AgentEvent, SessionConfig};
use crate::config::{Config, ExtensionConfig, GooseMode};
use crate::context_mgmt::format_message_for_compacting;
use crate::conversation::message::Message;
use crate::conversation::Conversation;
use crate::execution::manager::AgentManager;
use crate::providers;
use crate::providers::base::Provider;
use crate::session::extension_data::EnabledExtensionsState;
use crate::session::session_manager::{Session, SessionType};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeResult, JsonObject, ListToolsResult,
    ServerCapabilities, Tool,
};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub static EXTENSION_NAME: &str = "orchestrator";

struct CancelTokenGuard {
    manager: Arc<AgentManager>,
    session_id: String,
    disarmed: bool,
}

impl CancelTokenGuard {
    fn new(manager: Arc<AgentManager>, session_id: String) -> Self {
        Self {
            manager,
            session_id,
            disarmed: false,
        }
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for CancelTokenGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            let manager = self.manager.clone();
            let session_id = self.session_id.clone();
            tokio::spawn(async move {
                manager.unregister_cancel_token(&session_id).await;
            });
        }
    }
}

const DEFAULT_LIST_LIMIT: usize = 10;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ListSessionsParams {
    /// Filter by session type: "user", "sub_agent", "scheduled", "hidden", "terminal", "gateway".
    /// If omitted, returns all session types.
    session_type: Option<String>,
    /// Maximum number of sessions to return (most recent first). Defaults to 10.
    last_n: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ViewSessionParams {
    /// The session ID to inspect
    session_id: String,
    /// How to view the conversation: "first_last" returns the first and last message,
    /// "summarize" calls the LLM to produce a summary. If omitted, returns first and last.
    mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct StartAgentParams {
    /// Working directory for the new agent session
    working_dir: String,
    /// Human-readable name for the session
    name: Option<String>,
    // TODO: add a "model_tier" parameter (e.g. "fast" vs "normal") to let the orchestrator
    // choose between a fast/cheap model and the default one. For now we inherit the
    // orchestrator's own provider and model.
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct SendMessageParams {
    /// The session ID of the agent to send a message to
    session_id: String,
    /// The message text to send
    message: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct InterruptAgentParams {
    /// The session ID of the agent to interrupt
    session_id: String,
}

pub struct OrchestratorClient {
    info: InitializeResult,
    context: PlatformExtensionContext,
}

impl OrchestratorClient {
    pub fn new(context: PlatformExtensionContext) -> Result<Self> {
        let info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(EXTENSION_NAME, "1.0.0").with_title("Orchestrator"),
            )
            .with_instructions(
                "Manage agent sessions: list, view, start, send messages, and interrupt agents.",
            );

        Ok(Self { info, context })
    }

    async fn get_agent_manager(&self) -> Result<Arc<AgentManager>, String> {
        AgentManager::instance()
            .await
            .map_err(|e| format!("Failed to get agent manager: {}", e))
    }

    async fn get_provider(&self) -> Result<Arc<dyn Provider>, String> {
        let extension_manager = self
            .context
            .extension_manager
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or("Extension manager not available")?;

        let provider_guard = extension_manager.get_provider().lock().await;
        provider_guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "Provider not available".to_string())
    }

    async fn parent_model_config(
        &self,
        provider_name: &str,
    ) -> Result<goose_providers::model::ModelConfig, String> {
        if let Some(session) = self.context.session.as_ref() {
            return self.context.model_config_for_session(&session.id).await;
        }

        let model_name = Config::global()
            .get_goose_model()
            .map_err(|_| "Could not resolve model config: missing model".to_string())?;
        crate::model_config::model_config_from_user_config(provider_name, &model_name)
            .map_err(|e| format!("Could not resolve model config: {e}"))
    }

    fn parent_extensions(&self) -> Vec<ExtensionConfig> {
        let extension_data = self.context.session.as_ref().map(|s| &s.extension_data);
        EnabledExtensionsState::extensions_or_default(extension_data, Config::global())
    }

    async fn handle_list_sessions(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let type_filter = arguments
            .as_ref()
            .and_then(|args| args.get("session_type"))
            .and_then(|v| v.as_str());

        let limit = arguments
            .as_ref()
            .and_then(|args| args.get("last_n"))
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_LIST_LIMIT);

        let manager = self.get_agent_manager().await?;

        let mut sessions = if let Some(type_str) = type_filter {
            let session_type: SessionType = type_str
                .parse()
                .map_err(|e| format!("Invalid session type '{}': {}", type_str, e))?;
            self.context
                .session_manager
                .list_sessions_by_types(&[session_type])
                .await
                .map_err(|e| format!("Failed to list sessions: {}", e))?
        } else {
            self.context
                .session_manager
                .list_sessions()
                .await
                .map_err(|e| format!("Failed to list sessions: {}", e))?
        };

        // Most recent first
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        let total = sessions.len();
        sessions.truncate(limit);

        if sessions.is_empty() {
            return Ok(CallToolResult::success(vec![ContentBlock::text(
                "No sessions found.",
            )]));
        }

        let active_ids = manager.list_active_session_ids().await;

        let mut lines = vec![format!(
            "Showing {} of {} session(s):\n",
            sessions.len(),
            total
        )];
        for session in &sessions {
            let is_loaded = active_ids.contains(&session.id);
            let is_busy = if is_loaded {
                manager.is_session_busy(&session.id).await
            } else {
                false
            };

            let status = if is_busy {
                "🔄 busy"
            } else if is_loaded {
                "✓ loaded"
            } else {
                "○ idle"
            };

            lines.push(format!(
                "- **{}** ({})\n  Type: {} | Status: {} | Messages: {} | Updated: {}",
                session.name,
                session.id,
                session.session_type,
                status,
                session.message_count,
                session.updated_at.format("%Y-%m-%d %H:%M"),
            ));
        }

        Ok(CallToolResult::success(vec![ContentBlock::text(
            lines.join("\n"),
        )]))
    }

    async fn handle_view_session(
        &self,
        session_id_for_llm: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;
        let session_id = extract_string(&args, "session_id")?;
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("first_last");

        let session = self
            .context
            .session_manager
            .get_session(&session_id, true)
            .await
            .map_err(|e| format!("Session '{}' not found: {}", session_id, e))?;

        let manager = self.get_agent_manager().await?;
        let is_busy = manager.is_session_busy(&session_id).await;

        let mut output = vec![format!(
            "# Session: {} ({})\n\nType: {} | Status: {} | Working dir: {}\nMessages: {} | Updated: {}\n",
            session.name,
            session.id,
            session.session_type,
            if is_busy { "🔄 busy" } else { "idle" },
            session.working_dir.display(),
            session.message_count,
            session.updated_at.format("%Y-%m-%d %H:%M"),
        )];

        match mode {
            "first_last" => {
                if let Some(conversation) = &session.conversation {
                    let messages = agent_visible_session_messages(conversation);
                    if messages.is_empty() {
                        output.push("No messages in this session.".to_string());
                    } else {
                        output.push("## First message\n".to_string());
                        output.push(format_message_for_compacting(&messages[0]));

                        if messages.len() > 1 {
                            output.push(format!("\n*({} messages omitted)*\n", messages.len() - 2));
                            output.push("## Last message\n".to_string());
                            output
                                .push(format_message_for_compacting(&messages[messages.len() - 1]));
                        }
                    }
                } else {
                    output.push("No messages in this session.".to_string());
                }
            }
            "summarize" => {
                if let Some(conversation) = &session.conversation {
                    let messages = conversation.messages();
                    if messages.is_empty() {
                        output.push("No messages to summarize.".to_string());
                    } else {
                        let summary = self
                            .summarize_conversation(session_id_for_llm, messages)
                            .await?;
                        output.push(format!("## Summary\n\n{}", summary));
                    }
                } else {
                    output.push("No messages to summarize.".to_string());
                }
            }
            other => {
                return Err(format!(
                    "Unknown mode '{}'. Use 'first_last' or 'summarize'.",
                    other
                ));
            }
        }

        Ok(CallToolResult::success(vec![ContentBlock::text(
            output.join("\n"),
        )]))
    }

    async fn summarize_conversation(
        &self,
        session_id: &str,
        messages: &[Message],
    ) -> Result<String, String> {
        let provider = self.get_provider().await?;

        let conversation_text = agent_visible_session_messages(&Conversation::new_unvalidated(
            messages.iter().cloned(),
        ))
        .iter()
        .map(format_message_for_compacting)
        .collect::<Vec<_>>()
        .join("\n");

        let system =
            "You are a helpful assistant. Summarize the following conversation concisely, \
                       capturing the key topics, decisions, and current state. Be brief.";

        let user_message = Message::user().with_text(format!(
            "Summarize this conversation ({} messages):\n\n{}",
            messages.len(),
            conversation_text
        ));

        let model_config = self.parent_model_config(provider.get_name()).await?;
        let (response, _usage) = crate::model_config::complete_one_shot(
            provider.as_ref(),
            &model_config,
            session_id,
            system,
            &[user_message],
            &[],
        )
        .await
        .map_err(|e| format!("LLM summarization failed: {}", e))?;

        Ok(response
            .content
            .iter()
            .filter_map(|c| {
                if let crate::conversation::message::MessageContent::Text(t) = c {
                    Some(t.text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn handle_start_agent(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        self.authorize_start_agent(session_id).await?;

        let args = arguments.ok_or("Missing arguments")?;
        let working_dir = extract_string(&args, "working_dir")?;
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Orchestrated Agent")
            .to_string();

        let raw_path = PathBuf::from(&working_dir);
        let path = if raw_path.is_absolute() {
            raw_path
        } else {
            let base = self
                .context
                .session
                .as_ref()
                .map(|s| s.working_dir.clone())
                .unwrap_or_else(|| PathBuf::from("."));
            base.join(&raw_path)
        };

        let path = path
            .canonicalize()
            .map_err(|e| format!("Invalid working directory '{}': {}", working_dir, e))?;

        if !path.is_dir() {
            return Err(format!("'{}' is not a directory", working_dir));
        }

        let mode = GooseMode::default();

        let session = self
            .context
            .session_manager
            .create_session(path, name.clone(), SessionType::User, mode)
            .await
            .map_err(|e| format!("Failed to create session: {}", e))?;

        let manager = self.get_agent_manager().await?;
        let agent = manager
            .get_or_create_agent(session.id.clone())
            .await
            .map_err(|e| format!("Failed to create agent: {}", e))?;

        let parent_provider = self.get_provider().await?;
        let extensions = self.parent_extensions();
        let model_config = self.parent_model_config(parent_provider.get_name()).await?;
        let provider = providers::create(parent_provider.get_name(), extensions)
            .await
            .map_err(|e| format!("Failed to create provider for new agent: {}", e))?;
        agent
            .update_provider(provider, model_config, &session.id)
            .await
            .map_err(|e| format!("Failed to set provider on new agent: {}", e))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "Started agent session '{}' with ID: {}\n\nUse send_message with this session_id to interact with it.",
            name, session.id
        ))]))
    }

    async fn authorize_start_agent(&self, session_id: &str) -> Result<(), String> {
        if self.caller_session(session_id).await?.session_type == SessionType::SubAgent {
            return Err("Delegated tasks cannot start agent sessions".to_string());
        }
        Ok(())
    }

    async fn caller_session(&self, session_id: &str) -> Result<Session, String> {
        self.context
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|error| format!("Failed to get caller session: {error}"))
    }

    async fn authorize_send_message(
        &self,
        caller_session_id: &str,
        target_session_id: &str,
    ) -> Result<(), String> {
        let caller = self.caller_session(caller_session_id).await?;

        if target_session_id == caller_session_id {
            return Err("Cannot send a message to the orchestrator's own session".into());
        }

        if caller.session_type != SessionType::SubAgent {
            return Ok(());
        }

        let caller_parent_id = caller
            .parent_session_id
            .as_deref()
            .ok_or("Delegated tasks without a parent session cannot send messages")?;
        let target = self
            .context
            .session_manager
            .get_session(target_session_id, false)
            .await
            .map_err(|error| format!("Failed to get target session: {error}"))?;
        if target.session_type != SessionType::SubAgent
            || target.parent_session_id.as_deref() != Some(caller_parent_id)
        {
            return Err(
                "Delegated tasks can only send messages to sibling delegated sessions".into(),
            );
        }

        Ok(())
    }

    async fn handle_send_message(
        &self,
        parent_session_id: &str,
        parent_cancel: &CancellationToken,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;
        let session_id = extract_string(&args, "session_id")?;
        let message_text = extract_string(&args, "message")?;

        self.authorize_send_message(parent_session_id, &session_id)
            .await?;

        let manager = self.get_agent_manager().await?;

        let agent = manager
            .get_or_create_agent(session_id.clone())
            .await
            .map_err(|e| format!("Failed to get agent for session '{}': {}", session_id, e))?;

        if agent.provider().await.is_err() {
            if let Ok(parent_provider) = self.get_provider().await {
                let extensions = self.parent_extensions();
                let model_config = self.parent_model_config(parent_provider.get_name()).await?;
                if let Ok(provider) =
                    providers::create(parent_provider.get_name(), extensions).await
                {
                    agent
                        .update_provider(provider, model_config, &session_id)
                        .await
                        .map_err(|e| format!("Failed to set provider: {}", e))?;
                }
            }
        }

        let cancel_token = CancellationToken::new();
        manager
            .try_register_cancel_token(&session_id, cancel_token.clone())
            .await
            .map_err(|_| {
                format!(
                    "Session '{}' is currently busy. Use interrupt_agent first, or wait.",
                    session_id
                )
            })?;

        let mut guard = CancelTokenGuard::new(manager.clone(), session_id.clone());

        let user_message = Message::user().with_text(&message_text);
        let session_config = SessionConfig {
            id: session_id.clone(),
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };

        let mut stream = agent
            .reply(
                user_message,
                session_config,
                crate::agents::state_machine::enabled(),
                Some(cancel_token.clone()),
            )
            .await
            .map_err(|e| format!("Failed to start reply: {}", e))?;

        let mut response_parts: Vec<String> = Vec::new();
        let mut cancelled = false;

        loop {
            tokio::select! {
                _ = parent_cancel.cancelled() => {
                    cancel_token.cancel();
                    cancelled = true;
                    break;
                }
                event = stream.next() => {
                    match event {
                        Some(Ok(AgentEvent::Message(msg))) => {
                            let text = msg.as_concat_text();
                            if !text.is_empty() {
                                response_parts.push(text);
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            response_parts.push(format!("Error during agent processing: {}", e));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        drop(stream);
        guard.disarm();
        manager.unregister_cancel_token(&session_id).await;

        if cancelled {
            return Err("Cancelled by parent session".into());
        }

        if response_parts.is_empty() {
            Ok(CallToolResult::success(vec![ContentBlock::text(
                "Agent completed without producing text output.",
            )]))
        } else {
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "## Response from session {}\n\n{}",
                session_id,
                response_parts.join("\n\n")
            ))]))
        }
    }

    async fn handle_interrupt_agent(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;
        let session_id = extract_string(&args, "session_id")?;

        let manager = self.get_agent_manager().await?;

        manager
            .cancel_session(&session_id)
            .await
            .map_err(|e| format!("Failed to interrupt session '{}': {}", session_id, e))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "Interrupted agent session '{}'.",
            session_id
        ))]))
    }
}

fn agent_visible_session_messages(conversation: &Conversation) -> Vec<Message> {
    conversation
        .agent_visible_messages()
        .into_iter()
        .filter(|message| !message.is_turn_context())
        .collect()
}

#[async_trait]
impl McpClientTrait for OrchestratorClient {
    async fn list_tools(
        &self,
        session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        let mut tools = vec![
            Tool::new(
                "list_sessions".to_string(),
                "List agent sessions with their status (loaded, busy, idle). Returns the most recent 10 by default. Optionally filter by session type."
                    .to_string(),
                schema::<ListSessionsParams>(),
            ),
            Tool::new(
                "view_session".to_string(),
                "View a session's details and conversation. Mode 'first_last' (default) returns the first and last message. Mode 'summarize' calls the LLM to produce a conversation summary."
                    .to_string(),
                schema::<ViewSessionParams>(),
            ),
        ];

        if self.authorize_start_agent(session_id).await.is_ok() {
            tools.push(Tool::new(
                "start_agent".to_string(),
                "Start a new agent session with its own working directory. Inherits the current provider and model. Returns a session_id for future interaction."
                    .to_string(),
                schema::<StartAgentParams>(),
            ));
        }

        tools.extend([
            Tool::new(
                "send_message".to_string(),
                "Send a message to an existing agent session and get the response. Returns an error if the agent is currently busy."
                    .to_string(),
                schema::<SendMessageParams>(),
            ),
            Tool::new(
                "interrupt_agent".to_string(),
                "Interrupt a busy agent by cancelling its current operation."
                    .to_string(),
                schema::<InterruptAgentParams>(),
            ),
        ]);

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancel_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let result = match name {
            "list_sessions" => self.handle_list_sessions(arguments).await,
            "view_session" => self.handle_view_session(&ctx.session_id, arguments).await,
            "start_agent" => self.handle_start_agent(&ctx.session_id, arguments).await,
            "send_message" => {
                self.handle_send_message(&ctx.session_id, &cancel_token, arguments)
                    .await
            }
            "interrupt_agent" => self.handle_interrupt_agent(arguments).await,
            _ => Err(format!("Unknown tool: {}", name)),
        };

        match result {
            Ok(result) => Ok(result),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Error: {}",
                error
            ))])),
        }
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }
}

fn schema<T: JsonSchema>() -> JsonObject {
    let mut obj = serde_json::to_value(schema_for!(T))
        .map(|v| v.as_object().unwrap().clone())
        .expect("valid schema");
    obj.entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    obj
}

fn extract_string(args: &JsonObject, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Missing or invalid '{}'", key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::MessageContent;
    use crate::session::SessionManager;
    use rmcp::model::{Annotations, Role, TextContent};

    fn client_for(
        session_manager: Arc<SessionManager>,
        session: Option<crate::session::Session>,
    ) -> OrchestratorClient {
        OrchestratorClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager,
            scheduler: None,
            session: session.map(Arc::new),
            use_login_shell_path: false,
        })
        .unwrap()
    }

    async fn create_session(
        session_manager: &SessionManager,
        working_dir: &std::path::Path,
        session_type: SessionType,
    ) -> crate::session::Session {
        session_manager
            .create_session(
                working_dir.to_path_buf(),
                "orchestrator test".to_string(),
                session_type,
                GooseMode::default(),
            )
            .await
            .unwrap()
    }

    async fn create_subagent_session(
        session_manager: &SessionManager,
        working_dir: &std::path::Path,
        parent_session_id: &str,
    ) -> crate::session::Session {
        let session = create_session(session_manager, working_dir, SessionType::SubAgent).await;
        session_manager
            .update(&session.id)
            .parent_session_id(Some(parent_session_id.to_string()))
            .apply()
            .await
            .unwrap();
        session_manager
            .get_session(&session.id, false)
            .await
            .unwrap()
    }

    async fn start_agent(
        client: &OrchestratorClient,
        session_id: &str,
        working_dir: &std::path::Path,
    ) -> CallToolResult {
        let arguments = serde_json::json!({
            "working_dir": working_dir.to_string_lossy(),
        })
        .as_object()
        .unwrap()
        .clone();
        client
            .call_tool(
                &ToolCallContext::new(session_id.to_string(), None, None),
                "start_agent",
                Some(arguments),
                CancellationToken::default(),
            )
            .await
            .unwrap()
    }

    fn send_message_arguments(session_id: &str) -> JsonObject {
        serde_json::json!({
            "session_id": session_id,
            "message": "invoke a privileged tool",
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn first_last_projection_drops_hidden_endpoints_and_content() {
        let user_only = |text: &str| {
            MessageContent::Text(
                TextContent::new(text)
                    .with_annotations(Annotations::default().with_audience(vec![Role::User])),
            )
        };
        let conversation = Conversation::new_unvalidated([
            Message::assistant().with_content(user_only("hidden first")),
            Message::user().with_text("visible first"),
            Message::assistant()
                .with_content(user_only("hidden block"))
                .with_text("visible last"),
            Message::assistant().with_content(user_only("hidden last")),
        ]);

        let messages = agent_visible_session_messages(&conversation);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].as_concat_text(), "visible first");
        assert_eq!(messages[1].as_concat_text(), "visible last");
    }

    #[tokio::test]
    async fn subagent_direct_call_cannot_persist_peer_user_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let subagent =
            create_session(&session_manager, temp_dir.path(), SessionType::SubAgent).await;
        let client = client_for(Arc::clone(&session_manager), Some(subagent.clone()));

        let result = start_agent(&client, &subagent.id, temp_dir.path()).await;

        assert!(result.is_error.unwrap_or(false));
        assert!(session_manager
            .list_sessions_by_types(&[SessionType::User])
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn start_agent_listing_is_scoped_to_authorized_callers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let user = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let subagent =
            create_session(&session_manager, temp_dir.path(), SessionType::SubAgent).await;
        let client = client_for(Arc::clone(&session_manager), Some(user.clone()));

        assert!(client.authorize_start_agent(&user.id).await.is_ok());
        let user_tools = client
            .list_tools(&user.id, None, CancellationToken::default())
            .await
            .unwrap();
        assert!(user_tools
            .tools
            .iter()
            .any(|tool| tool.name == "start_agent"));

        for session_id in [&subagent.id, "missing-session"] {
            let tools = client
                .list_tools(session_id, None, CancellationToken::default())
                .await
                .unwrap();
            assert!(tools.tools.iter().all(|tool| tool.name != "start_agent"));
            assert!(tools.tools.iter().any(|tool| tool.name == "send_message"));
        }

        assert!(client
            .authorize_start_agent("missing-session")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn unknown_caller_cannot_persist_user_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let client = client_for(Arc::clone(&session_manager), None);

        let result = start_agent(&client, "missing-session", temp_dir.path()).await;

        assert!(result.is_error.unwrap_or(false));
        assert!(session_manager
            .list_sessions_by_types(&[SessionType::User])
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn subagent_cannot_send_message_to_parent_user_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let user = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let subagent = create_subagent_session(&session_manager, temp_dir.path(), &user.id).await;
        let client = client_for(Arc::clone(&session_manager), Some(subagent.clone()));

        let error = client
            .handle_send_message(
                &subagent.id,
                &CancellationToken::default(),
                Some(send_message_arguments(&user.id)),
            )
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "Delegated tasks can only send messages to sibling delegated sessions"
        );
        assert_eq!(
            session_manager
                .get_session(&user.id, true)
                .await
                .unwrap()
                .message_count,
            0
        );
    }

    #[tokio::test]
    async fn subagent_can_send_message_to_sibling_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let parent = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let caller = create_subagent_session(&session_manager, temp_dir.path(), &parent.id).await;
        let target = create_subagent_session(&session_manager, temp_dir.path(), &parent.id).await;
        let client = client_for(Arc::clone(&session_manager), Some(caller.clone()));

        assert!(client
            .authorize_send_message(&caller.id, &target.id)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn subagent_cannot_send_message_across_delegation_trees() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let caller_parent =
            create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let target_parent =
            create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let caller =
            create_subagent_session(&session_manager, temp_dir.path(), &caller_parent.id).await;
        let target =
            create_subagent_session(&session_manager, temp_dir.path(), &target_parent.id).await;
        let client = client_for(Arc::clone(&session_manager), Some(caller.clone()));

        let error = client
            .handle_send_message(
                &caller.id,
                &CancellationToken::default(),
                Some(send_message_arguments(&target.id)),
            )
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "Delegated tasks can only send messages to sibling delegated sessions"
        );
        assert_eq!(
            session_manager
                .get_session(&target.id, true)
                .await
                .unwrap()
                .message_count,
            0
        );
    }

    #[tokio::test]
    async fn subagent_cannot_send_message_to_descendant_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let parent = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let caller = create_subagent_session(&session_manager, temp_dir.path(), &parent.id).await;
        let descendant =
            create_subagent_session(&session_manager, temp_dir.path(), &caller.id).await;
        let client = client_for(Arc::clone(&session_manager), Some(caller.clone()));

        let error = client
            .authorize_send_message(&caller.id, &descendant.id)
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "Delegated tasks can only send messages to sibling delegated sessions"
        );
    }

    #[tokio::test]
    async fn subagent_without_parent_cannot_send_message() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let caller = create_session(&session_manager, temp_dir.path(), SessionType::SubAgent).await;
        let target = create_session(&session_manager, temp_dir.path(), SessionType::SubAgent).await;
        let client = client_for(Arc::clone(&session_manager), Some(caller.clone()));

        let error = client
            .authorize_send_message(&caller.id, &target.id)
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "Delegated tasks without a parent session cannot send messages"
        );
    }

    #[tokio::test]
    async fn user_can_send_message_to_user_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let caller = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let target = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let client = client_for(Arc::clone(&session_manager), Some(caller.clone()));

        assert!(client
            .authorize_send_message(&caller.id, &target.id)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn unknown_caller_cannot_send_message() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let target = create_session(&session_manager, temp_dir.path(), SessionType::User).await;
        let client = client_for(Arc::clone(&session_manager), None);

        let error = client
            .authorize_send_message("missing-session", &target.id)
            .await
            .unwrap_err();

        assert!(error.starts_with("Failed to get caller session:"));
    }
}
