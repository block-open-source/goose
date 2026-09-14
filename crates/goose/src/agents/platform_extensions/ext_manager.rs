use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::tool_execution::ToolCallContext;
use crate::config::get_extension_by_name;
use crate::session::SessionType;
use anyhow::Result;
use async_trait::async_trait;
use indoc::indoc;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorCode, ErrorData, GetPromptResult, Implementation,
    InitializeResult, JsonObject, ListPromptsResult, ListResourcesResult, ListToolsResult,
    ReadResourceResult, ServerCapabilities, ServerNotification, Tool, ToolAnnotations,
};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub static EXTENSION_NAME: &str = "Extension Manager";

#[derive(Debug, thiserror::Error)]
pub enum ExtensionManagerToolError {
    #[error("Unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },

    #[error("Extension manager not available")]
    ManagerUnavailable,

    #[error("Missing required parameter: {param_name}")]
    MissingParameter { param_name: String },

    #[error("Extension operation failed: {message}")]
    OperationFailed { message: String },

    #[error("Failed to deserialize parameters: {0}")]
    DeserializationError(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ManageExtensionAction {
    Enable,
    Disable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManageExtensionsParams {
    pub action: ManageExtensionAction,
    pub extension_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadResourceParams {
    pub uri: String,
    pub extension_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListResourcesParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_name: Option<String>,
}

pub const READ_RESOURCE_TOOL_NAME: &str = "read_resource";
pub const LIST_RESOURCES_TOOL_NAME: &str = "list_resources";
pub const SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME: &str = "search_available_extensions";
pub const MANAGE_EXTENSIONS_TOOL_NAME: &str = "manage_extensions";
pub const MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE: &str = "extensionmanager__manage_extensions";

pub struct ExtensionManagerClient {
    info: InitializeResult,
    #[allow(dead_code)]
    context: PlatformExtensionContext,
}

impl ExtensionManagerClient {
    pub fn new(context: PlatformExtensionContext) -> Result<Self> {
        let info = InitializeResult::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(Implementation::new(EXTENSION_NAME, "1.0.0").with_title(EXTENSION_NAME))
        .with_instructions(indoc! {r#"
            Extension Management

            Use these tools to discover, enable, and disable extensions, as well as review resources.

            Available tools:
            - search_available_extensions: Find extensions available to enable/disable
            - manage_extensions: Enable or disable extensions
            - list_resources: List resources from extensions
            - read_resource: Read specific resources from extensions

            When you lack the tools needed to complete a task, use search_available_extensions first
            to discover what extensions can help.

            Use manage_extensions to enable or disable specific extensions by name.
            Use list_resources and read_resource to work with extension data and resources.
        "#});

        Ok(Self { info, context })
    }

    async fn handle_search_available_extensions(
        &self,
    ) -> Result<Vec<ContentBlock>, ExtensionManagerToolError> {
        if let Some(weak_ref) = &self.context.extension_manager {
            if let Some(extension_manager) = weak_ref.upgrade() {
                match extension_manager.search_available_extensions().await {
                    Ok(content) => Ok(content),
                    Err(e) => Err(ExtensionManagerToolError::OperationFailed {
                        message: format!("Failed to search available extensions: {}", e.message),
                    }),
                }
            } else {
                Err(ExtensionManagerToolError::ManagerUnavailable)
            }
        } else {
            Err(ExtensionManagerToolError::ManagerUnavailable)
        }
    }

    async fn handle_manage_extensions(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<Vec<ContentBlock>, ExtensionManagerToolError> {
        let arguments = arguments.ok_or(ExtensionManagerToolError::MissingParameter {
            param_name: "arguments".to_string(),
        })?;

        let params: ManageExtensionsParams =
            serde_json::from_value(serde_json::Value::Object(arguments))?;

        match self
            .manage_extensions_impl(session_id, params.action, params.extension_name)
            .await
        {
            Ok(content) => Ok(content),
            Err(error_data) => Err(ExtensionManagerToolError::OperationFailed {
                message: error_data.message.to_string(),
            }),
        }
    }

    async fn manage_extensions_impl(
        &self,
        session_id: &str,
        action: ManageExtensionAction,
        extension_name: String,
    ) -> Result<Vec<ContentBlock>, ErrorData> {
        let session = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|error| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to get caller session: {error}"),
                    None,
                )
            })?;
        if session.session_type == SessionType::SubAgent {
            return Err(ErrorData::new(
                ErrorCode::INVALID_REQUEST,
                "Subagents cannot manage extensions".to_string(),
                None,
            ));
        }

        let extension_manager = self
            .context
            .extension_manager
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Extension manager is no longer available".to_string(),
                    None,
                )
            })?;

        if action == ManageExtensionAction::Disable {
            return extension_manager
                .remove_extension(&extension_name)
                .await
                .map(|_| {
                    vec![ContentBlock::text(format!(
                        "The extension '{}' has been disabled successfully",
                        extension_name
                    ))]
                })
                .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None));
        }

        let config = match get_extension_by_name(&extension_name) {
            Some(config) => config,
            None => {
                return Err(ErrorData::new(
                    ErrorCode::RESOURCE_NOT_FOUND,
                    format!(
                        "Extension '{}' not found. Please check the extension name and try again.",
                        extension_name
                    ),
                    None,
                ));
            }
        };

        extension_manager
            .add_extension(config, None, None, None)
            .await
            .map(|_| {
                vec![ContentBlock::text(format!(
                    "The extension '{}' has been installed successfully",
                    extension_name
                ))]
            })
            .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))
    }

    async fn handle_list_resources(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<Vec<ContentBlock>, ExtensionManagerToolError> {
        if let Some(weak_ref) = &self.context.extension_manager {
            if let Some(extension_manager) = weak_ref.upgrade() {
                let params = arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                match extension_manager
                    .list_resources(
                        session_id,
                        params,
                        tokio_util::sync::CancellationToken::default(),
                    )
                    .await
                {
                    Ok(content) => Ok(content),
                    Err(e) => Err(ExtensionManagerToolError::OperationFailed {
                        message: format!("Failed to list resources: {}", e.message),
                    }),
                }
            } else {
                Err(ExtensionManagerToolError::ManagerUnavailable)
            }
        } else {
            Err(ExtensionManagerToolError::ManagerUnavailable)
        }
    }

    async fn handle_read_resource(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<Vec<ContentBlock>, ExtensionManagerToolError> {
        if let Some(weak_ref) = &self.context.extension_manager {
            if let Some(extension_manager) = weak_ref.upgrade() {
                let params = arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                match extension_manager
                    .read_resource_tool(
                        session_id,
                        params,
                        tokio_util::sync::CancellationToken::default(),
                    )
                    .await
                {
                    Ok(content) => Ok(content),
                    Err(e) => Err(ExtensionManagerToolError::OperationFailed {
                        message: format!("Failed to read resource: {}", e.message),
                    }),
                }
            } else {
                Err(ExtensionManagerToolError::ManagerUnavailable)
            }
        } else {
            Err(ExtensionManagerToolError::ManagerUnavailable)
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn get_tools(&self, include_manage_extensions: bool) -> Vec<Tool> {
        let mut tools = vec![Tool::new(
                SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME.to_string(),
                "Searches for additional extensions available to help complete tasks.
        Use this tool when you're unable to find a specific feature or functionality you need to complete your task, or when standard approaches aren't working.
        These extensions might provide the exact tools needed to solve your problem.
        If you find a relevant one, consider using your tools to enable it.".to_string(),
                Arc::new(
                    serde_json::json!({
                        "type": "object",
                        "required": [],
                        "properties": {}
                    })
                    .as_object()
                    .expect("Schema must be an object")
                    .clone()
                ),
            ).annotate(ToolAnnotations::from_raw(
                Some("Discover extensions".to_string()),
                Some(true),
                Some(false),
                Some(false),
                Some(false),
            ))];

        if include_manage_extensions {
            tools.push(
                Tool::new(
                    MANAGE_EXTENSIONS_TOOL_NAME.to_string(),
                    "Tool to manage extensions and tools in goose context.
            Enable or disable extensions to help complete tasks.
            Enable or disable an extension by providing the extension name.
            "
                    .to_string(),
                    Arc::new(
                        serde_json::to_value(schema_for!(ManageExtensionsParams))
                            .expect("Failed to serialize schema")
                            .as_object()
                            .expect("Schema must be an object")
                            .clone(),
                    ),
                )
                .annotate(ToolAnnotations::from_raw(
                    Some("Enable or disable an extension".to_string()),
                    Some(false),
                    Some(false),
                    Some(false),
                    Some(false),
                )),
            );
        }

        if let Some(weak_ref) = &self.context.extension_manager {
            if let Some(extension_manager) = weak_ref.upgrade() {
                if extension_manager.supports_resources().await {
                    tools.extend([
                        Tool::new(
                            LIST_RESOURCES_TOOL_NAME.to_string(),
                            indoc! {r#"
            List resources from an extension(s).

            Resources allow extensions to share data that provide context to LLMs, such as
            files, database schemas, or application-specific information. This tool lists resources
            in the provided extension, and returns a list for the user to browse. If no extension
            is provided, the tool will search all extensions for the resource.
        "#}
                            .to_string(),
                            Arc::new(
                                serde_json::to_value(schema_for!(ListResourcesParams))
                                    .expect("Failed to serialize schema")
                                    .as_object()
                                    .expect("Schema must be an object")
                                    .clone(),
                            ),
                        )
                        .annotate(ToolAnnotations::from_raw(
                            Some("List resources".to_string()),
                            Some(true),
                            Some(false),
                            Some(false),
                            Some(false),
                        )),
                        Tool::new(
                            READ_RESOURCE_TOOL_NAME.to_string(),
                            indoc! {r#"
            Read a resource from a specific extension.

            Resources allow extensions to share data that provide context to LLMs, such as
            files, database schemas, or application-specific information. You must pass the
            owning extension as `extension_name`; if you don't know which extension owns a
            URI, call `list_resources` first — its output labels each resource with its
            extension.
        "#}
                            .to_string(),
                            Arc::new(
                                serde_json::to_value(schema_for!(ReadResourceParams))
                                    .expect("Failed to serialize schema")
                                    .as_object()
                                    .expect("Schema must be an object")
                                    .clone(),
                            ),
                        )
                        .annotate(ToolAnnotations::from_raw(
                            Some("Read a resource".to_string()),
                            Some(true),
                            Some(false),
                            Some(false),
                            Some(false),
                        )),
                    ]);
                }
            }
        }

        tools
    }
}

#[async_trait]
impl McpClientTrait for ExtensionManagerClient {
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
        // Extension manager doesn't expose resources directly
        Err(Error::TransportClosed)
    }

    async fn list_tools(
        &self,
        session_id: &str,
        _next_cursor: Option<String>,
        _cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        let can_manage_extensions = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .is_ok_and(|session| session.session_type != SessionType::SubAgent);

        Ok(ListToolsResult {
            tools: self.get_tools(can_manage_extensions).await,
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
        let result = match name {
            SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME => {
                self.handle_search_available_extensions().await
            }
            MANAGE_EXTENSIONS_TOOL_NAME => {
                self.handle_manage_extensions(session_id, arguments).await
            }
            LIST_RESOURCES_TOOL_NAME => self.handle_list_resources(session_id, arguments).await,
            READ_RESOURCE_TOOL_NAME => self.handle_read_resource(session_id, arguments).await,
            _ => Err(ExtensionManagerToolError::UnknownTool {
                tool_name: name.to_string(),
            }),
        };

        match result {
            Ok(content) => Ok(CallToolResult::success(content)),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(
                error.to_string(),
            )])),
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

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::extension_manager::ExtensionManager;
    use crate::config::GooseMode;
    use std::path::PathBuf;

    fn client_for(manager: &Arc<ExtensionManager>) -> ExtensionManagerClient {
        ExtensionManagerClient::new(PlatformExtensionContext {
            extension_manager: Some(Arc::downgrade(manager)),
            session_manager: manager.get_context().session_manager.clone(),
            scheduler: None,
            session: None,
            use_login_shell_path: false,
        })
        .unwrap()
    }

    async fn create_session(manager: &ExtensionManager, session_type: SessionType) -> String {
        manager
            .get_context()
            .session_manager
            .create_session(
                PathBuf::from("/tmp/extension-manager-test"),
                "extension manager test".to_string(),
                session_type,
                GooseMode::default(),
            )
            .await
            .unwrap()
            .id
    }

    fn manage_arguments(action: &str) -> JsonObject {
        serde_json::json!({
            "action": action,
            "extension_name": "developer",
        })
        .as_object()
        .unwrap()
        .clone()
    }

    async fn manage(
        client: &ExtensionManagerClient,
        session_id: &str,
        action: &str,
    ) -> CallToolResult {
        client
            .call_tool(
                &ToolCallContext::new(session_id.to_string(), None, None),
                MANAGE_EXTENSIONS_TOOL_NAME,
                Some(manage_arguments(action)),
                CancellationToken::default(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn subagent_direct_calls_cannot_enable_or_disable_extensions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(ExtensionManager::new_without_provider(
            temp_dir.path().to_path_buf(),
        ));
        let client = client_for(&manager);
        let user_id = create_session(&manager, SessionType::User).await;
        let subagent_id = create_session(&manager, SessionType::SubAgent).await;

        let enable = manage(&client, &subagent_id, "enable").await;
        assert!(enable.is_error.unwrap_or(false));
        assert!(!manager.is_extension_enabled("developer").await);

        let user_enable = manage(&client, &user_id, "enable").await;
        assert!(!user_enable.is_error.unwrap_or(false));
        assert!(manager.is_extension_enabled("developer").await);

        let disable = manage(&client, &subagent_id, "disable").await;
        assert!(disable.is_error.unwrap_or(false));
        assert!(manager.is_extension_enabled("developer").await);

        let user_disable = manage(&client, &user_id, "disable").await;
        assert!(!user_disable.is_error.unwrap_or(false));
        assert!(!manager.is_extension_enabled("developer").await);
    }

    #[tokio::test]
    async fn subagent_and_unknown_callers_are_not_offered_extension_management() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(ExtensionManager::new_without_provider(
            temp_dir.path().to_path_buf(),
        ));
        let client = client_for(&manager);
        let user_id = create_session(&manager, SessionType::User).await;
        let subagent_id = create_session(&manager, SessionType::SubAgent).await;

        let user_tools = client
            .list_tools(&user_id, None, CancellationToken::default())
            .await
            .unwrap();
        assert!(user_tools
            .tools
            .iter()
            .any(|tool| tool.name == MANAGE_EXTENSIONS_TOOL_NAME));

        for session_id in [&subagent_id, "missing-session"] {
            let tools = client
                .list_tools(session_id, None, CancellationToken::default())
                .await
                .unwrap();
            assert!(tools
                .tools
                .iter()
                .any(|tool| tool.name == SEARCH_AVAILABLE_EXTENSIONS_TOOL_NAME));
            assert!(tools
                .tools
                .iter()
                .all(|tool| tool.name != MANAGE_EXTENSIONS_TOOL_NAME));
        }

        let unknown_enable = manage(&client, "missing-session", "enable").await;
        assert!(unknown_enable.is_error.unwrap_or(false));
        assert!(!manager.is_extension_enabled("developer").await);
    }
}
