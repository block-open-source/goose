use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::tool_execution::ToolCallContext;
use crate::conversation::Conversation;
use crate::session::session_manager::SessionType;
use anyhow::Result;
use async_trait::async_trait;
use indoc::indoc;
use rmcp::model::{
    Annotations, CallToolResult, ContentBlock, Implementation, InitializeResult, JsonObject,
    ListToolsResult, Role, ServerCapabilities, TextContent, Tool, ToolAnnotations,
};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub static EXTENSION_NAME: &str = "chatrecall";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ChatRecallParams {
    /// Search keywords. Use multiple related terms/synonyms (e.g., 'database postgres sql'). Mutually exclusive with session_id.
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    /// Session ID to load. Returns first/last 3 messages. Mutually exclusive with query.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// Max results (default: 10, max: 50). Search mode only.
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
    /// ISO 8601 date (e.g., '2025-10-01T00:00:00Z'). Search mode only.
    #[serde(skip_serializing_if = "Option::is_none")]
    after_date: Option<String>,
    /// ISO 8601 date (e.g., '2025-10-15T23:59:59Z'). Search mode only.
    #[serde(skip_serializing_if = "Option::is_none")]
    before_date: Option<String>,
}

pub struct ChatRecallClient {
    info: InitializeResult,
    context: PlatformExtensionContext,
}

fn agent_only_history(text: String) -> ContentBlock {
    ContentBlock::Text(
        TextContent::new(text)
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
    )
}

fn format_agent_visible_excerpt(conversation: &Conversation) -> Option<(usize, String)> {
    let messages: Vec<_> = conversation
        .agent_visible_messages()
        .into_iter()
        .filter(|message| !message.is_turn_context())
        .collect();
    let total = messages.len();
    if total == 0 {
        return None;
    }

    let mut output = String::new();
    let first_count = std::cmp::min(3, total);
    output.push_str("--- First Few Messages ---\n\n");
    for (idx, message) in messages.iter().take(first_count).enumerate() {
        output.push_str(&format!("{}. [{:?}] ", idx + 1, message.role));
        for content in &message.content {
            if let Some(text) = content.as_text() {
                output.push_str(text);
                output.push('\n');
            }
        }
        output.push('\n');
    }

    if total > first_count {
        output.push_str("--- Last Few Messages ---\n\n");
        let last_count = std::cmp::min(3, total);
        let skip_count = total.saturating_sub(last_count);
        for (idx, message) in messages.iter().skip(skip_count).enumerate() {
            output.push_str(&format!("{}. [{:?}] ", skip_count + idx + 1, message.role));
            for content in &message.content {
                if let Some(text) = content.as_text() {
                    output.push_str(text);
                    output.push('\n');
                }
            }
            output.push('\n');
        }
    }

    Some((total, output))
}

impl ChatRecallClient {
    pub fn new(context: PlatformExtensionContext) -> Result<Self> {
        let info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(EXTENSION_NAME.to_string(), "1.0.0".to_string())
                    .with_title("Chat Recall"),
            )
            .with_instructions(indoc! {r#"
                Chat Recall

                Search past conversations and load session summaries when the user expects some memory or context.

                Two modes:
                - Search mode: Use query with keywords/synonyms to find relevant messages
                - Load mode: Use session_id to get first and last messages of a specific session
            "#}.to_string());

        Ok(Self { info, context })
    }

    fn search_session_types(&self) -> Vec<SessionType> {
        match self.context.session.as_ref().map(|s| s.session_type) {
            Some(SessionType::Acp) => vec![SessionType::Acp],
            _ => vec![SessionType::User, SessionType::Scheduled],
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_chatrecall(
        &self,
        current_session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<Vec<ContentBlock>, String> {
        let arguments = arguments.ok_or("Missing arguments")?;

        let target_session_id = arguments
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(sid) = target_session_id {
            // LOAD MODE: Get session summary (first and last few messages)
            match self.context.session_manager.get_session(&sid, true).await {
                Ok(loaded_session) => {
                    let conversation = loaded_session.conversation.as_ref();

                    if conversation.is_none() {
                        return Ok(vec![ContentBlock::text(format!(
                            "Session {} has no conversation.",
                            sid
                        ))]);
                    }

                    let Some((total, excerpt)) =
                        format_agent_visible_excerpt(conversation.unwrap())
                    else {
                        return Ok(vec![ContentBlock::text(format!(
                            "Session {} has no messages.",
                            sid
                        ))]);
                    };

                    let mut output = format!(
                        "Session: {} (ID: {})\nWorking Dir: {}\nTotal Messages: {}\n\n",
                        loaded_session.name,
                        sid,
                        loaded_session.working_dir.display(),
                        total
                    );

                    output.push_str(&excerpt);

                    Ok(vec![agent_only_history(output)])
                }
                Err(e) => Err(format!("Failed to load session: {}", e)),
            }
        } else {
            // SEARCH MODE: Search across all sessions
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or("Missing required parameter: query or session_id")?
                .to_string();

            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_i64())
                .map(|l| l as usize)
                .unwrap_or(10)
                .min(50);

            let after_date = arguments
                .get("after_date")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let before_date = arguments
                .get("before_date")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let exclude_session_id = Some(current_session_id.to_string());

            match self
                .context
                .session_manager
                .search_chat_history(
                    &query,
                    Some(limit),
                    after_date,
                    before_date,
                    exclude_session_id,
                    self.search_session_types(),
                )
                .await
            {
                Ok(results) => {
                    let formatted_results = if results.total_matches == 0 {
                        format!("No results found for query: '{}'", query)
                    } else {
                        let mut output = format!(
                            "Found {} matching message(s) across {} session(s) for query: '{}'\n\n",
                            results.total_matches,
                            results.results.len(),
                            query
                        );
                        for (idx, result) in results.results.iter().enumerate() {
                            output.push_str(&format!(
                                "{}. Session: {} (ID: {})\n   Working Dir: {}\n   Last Activity: {}\n   Showing {} of {} total message(s) in session:\n\n",
                                idx + 1,
                                result.session_description,
                                result.session_id,
                                result.session_working_dir,
                                result.last_activity.format("%Y-%m-%d"),
                                result.messages.len(),
                                result.total_messages_in_session
                            ));

                            for (msg_idx, message) in result.messages.iter().enumerate() {
                                output.push_str(&format!(
                                    "   {}.{} [{}]\n   {}\n\n",
                                    idx + 1,
                                    msg_idx + 1,
                                    message.role,
                                    message
                                        .content
                                        .lines()
                                        .map(|line| format!("   {}", line))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                ));
                            }
                        }
                        output
                    };
                    let content = if results.total_matches == 0 {
                        ContentBlock::text(formatted_results)
                    } else {
                        agent_only_history(formatted_results)
                    };
                    Ok(vec![content])
                }
                Err(e) => Err(format!("Chat recall failed: {}", e)),
            }
        }
    }

    fn get_tools() -> Vec<Tool> {
        let schema = schema_for!(ChatRecallParams);
        let schema_value =
            serde_json::to_value(schema).expect("Failed to serialize ChatRecallParams schema");

        let input_schema = schema_value
            .as_object()
            .expect("Schema should be an object")
            .clone();

        vec![Tool::new(
            "chatrecall".to_string(),
            indoc! {r#"
                Search past chat or load session summaries. Use when it is clear user expects some memory or context.

                search mode (query): Use multiple keywords/synonyms. Returns messages grouped by session, ordered by recency. Supports date filters.
                load mode (session_id): Returns first/last 3 messages of a session.
            "#}
            .to_string(),
            input_schema,
        )
        .annotate(ToolAnnotations::from_raw(
            Some("Recall past conversations".to_string()),
            Some(true),
            Some(false),
            Some(true),
            Some(false),
        ))]
    }
}

#[async_trait]
impl McpClientTrait for ChatRecallClient {
    async fn list_tools(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        Ok(ListToolsResult {
            tools: Self::get_tools(),
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
        _cancellation_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let session_id = &ctx.session_id;
        let content = match name {
            "chatrecall" => self.handle_chatrecall(session_id, arguments).await,
            _ => Err(format!("Unknown tool: {}", name)),
        };

        match content {
            Ok(content) => Ok(CallToolResult::success(content)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GooseMode;
    use crate::conversation::message::{Message, MessageContent, MessageMetadata};
    use crate::session::SessionManager;
    use std::sync::Arc;

    fn annotated_text(text: &str, audience: Vec<Role>) -> MessageContent {
        MessageContent::Text(
            TextContent::new(text).with_annotations(Annotations::default().with_audience(audience)),
        )
    }

    fn projected_tool_text(content: Vec<ContentBlock>, audience: Role) -> String {
        let message =
            Message::user().with_tool_response("chatrecall", Ok(CallToolResult::success(content)));
        let projected = match audience {
            Role::Assistant => message.agent_visible_content(),
            Role::User => message.user_visible_content(),
        };
        let Some(MessageContent::ToolResponse(response)) = projected.content.first() else {
            return String::new();
        };
        let Ok(result) = &response.tool_result else {
            return String::new();
        };

        result
            .content
            .iter()
            .filter_map(|content| content.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn loaded_excerpt_projects_audience_before_selecting_endpoints() {
        let conversation = Conversation::new_unvalidated([
            Message::user()
                .with_text("hidden first row")
                .with_metadata(MessageMetadata::user_only()),
            Message::user()
                .with_text("visible first")
                .with_content(annotated_text("user-only first secret", vec![Role::User])),
            Message::assistant().with_text("visible middle"),
            Message::assistant()
                .with_text("hidden last row")
                .with_metadata(MessageMetadata::user_only()),
            Message::user()
                .with_text("visible last")
                .with_content(annotated_text("user-only last secret", vec![Role::User])),
        ]);

        let (total, excerpt) = format_agent_visible_excerpt(&conversation).unwrap();

        assert_eq!(total, 3);
        assert!(excerpt.contains("visible first"));
        assert!(excerpt.contains("visible last"));
        assert!(!excerpt.contains("hidden first row"));
        assert!(!excerpt.contains("hidden last row"));
        assert!(!excerpt.contains("user-only first secret"));
        assert!(!excerpt.contains("user-only last secret"));
        let canonical_user_text = conversation
            .user_visible_messages()
            .iter()
            .map(Message::as_concat_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(canonical_user_text.contains("user-only first secret"));
        assert!(canonical_user_text.contains("user-only last secret"));
    }

    #[tokio::test]
    async fn history_results_remain_agent_visible_without_becoming_user_visible() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let current_session = session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "current".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();
        let target_session = session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "target".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();
        session_manager
            .add_message(
                &target_session.id,
                &Message::assistant()
                    .with_text("agent-only secret marker")
                    .with_metadata(MessageMetadata::agent_only()),
            )
            .await
            .unwrap();

        let client = ChatRecallClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager,
            scheduler: None,
            session: Some(Arc::new(current_session.clone())),
            use_login_shell_path: false,
        })
        .unwrap();
        let load_output = client
            .handle_chatrecall(
                &current_session.id,
                Some(
                    serde_json::json!({ "session_id": target_session.id })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        let search_output = client
            .handle_chatrecall(
                &current_session.id,
                Some(
                    serde_json::json!({ "query": "agent-only secret marker" })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();

        for output in [load_output, search_output] {
            assert!(projected_tool_text(output.clone(), Role::Assistant)
                .contains("agent-only secret marker"));
            assert!(!projected_tool_text(output, Role::User).contains("agent-only secret marker"));
        }
    }
}
