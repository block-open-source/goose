use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::reply_parts::{
    coerce_tool_arguments, prepare_tools_for_provider, toolshim_postprocess,
};
use crate::agents::tool_execution::ToolCallContext;
use crate::config::paths::Paths;
use crate::conversation::message::Message;
use crate::goose_apps::McpAppResource;
use crate::goose_apps::{GooseApp, McpAppCache, WindowProps};
use crate::prompt_template::render_template;
use crate::providers::base::{Provider, ProviderUsage};
use async_trait::async_trait;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeResult, JsonObject,
    ListResourcesResult, ListToolsResult, MetaObject, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, Tool as McpTool,
};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

async fn complete_app_content(
    provider: Arc<dyn Provider>,
    model_config: &goose_providers::model::ModelConfig,
    session_id: &str,
    system_prompt: String,
    messages: &[Message],
    tools: Vec<McpTool>,
) -> Result<(Message, ProviderUsage), String> {
    let (tools, toolshim_tools, system_prompt) =
        prepare_tools_for_provider(tools, system_prompt, model_config);

    let (response, usage) = crate::session_context::with_session_id(
        Some(session_id.to_string()),
        provider.complete(model_config, &system_prompt, messages, &tools),
    )
    .await
    .map_err(|e| format!("LLM call failed: {e}"))?;

    let response = if model_config.toolshim {
        toolshim_postprocess(response, &toolshim_tools)
            .await
            .map_err(|e| format!("Toolshim failed: {e}"))?
    } else {
        response
    };

    Ok((response, usage))
}

pub static EXTENSION_NAME: &str = "apps";
const CLOCK_APP_NAME: &str = "clock";
const CLOCK_HTML: &str = include_str!("../../goose_apps/clock.html");

const DEFAULT_WINDOW_PROPS: WindowProps = WindowProps {
    width: 800,
    height: 600,
    resizable: true,
};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct CreateAppParams {
    /// What the app should do - a description or PRD that will be used to generate the app
    prd: String,
}

/// Parameters for iterate_app tool
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct IterateAppParams {
    /// Name of the app to iterate on
    name: String,
    /// Feedback or requested changes to improve the app
    feedback: String,
}

/// Parameters for delete_app tool
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct DeleteAppParams {
    /// Name of the app to delete
    name: String,
}

/// Parameters for list_apps tool
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ListAppsParams {
    // No parameters needed - lists all apps
}

/// Response from create_app_content tool
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct CreateAppContentResponse {
    /// App name (lowercase, hyphens allowed, no spaces)
    name: String,
    /// Brief description of what the app does (1-2 sentences, max 100 chars)
    description: String,
    /// Complete HTML code for the app, from <!DOCTYPE html> to </html>
    html: String,
    /// Window width in pixels (recommended: 400-1600)
    width: Option<u32>,
    /// Window height in pixels (recommended: 300-1200)
    height: Option<u32>,
    /// Whether the window should be resizable
    resizable: Option<bool>,
}

/// Response from update_app_content tool
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct UpdateAppContentResponse {
    /// Updated description of what the app does (1-2 sentences, max 100 chars)
    description: String,
    /// Complete updated HTML code for the app, from <!DOCTYPE html> to </html>
    html: String,
    /// Updated PRD reflecting the current state of the app after this iteration
    prd: String,
    /// Updated window width in pixels (optional - only if size should change)
    width: Option<u32>,
    /// Updated window height in pixels (optional - only if size should change)
    height: Option<u32>,
    /// Updated resizable property (optional - only if it should change)
    resizable: Option<bool>,
}

pub struct AppsManagerClient {
    info: InitializeResult,
    context: PlatformExtensionContext,
    apps_dir: PathBuf,
}

impl AppsManagerClient {
    pub fn new(context: PlatformExtensionContext) -> Result<Self, String> {
        let apps_dir = Paths::in_data_dir(EXTENSION_NAME);

        fs::create_dir_all(&apps_dir)
            .map_err(|e| format!("Failed to create apps directory: {}", e))?;

        let client = Self {
            info: Self::create_info(),
            context,
            apps_dir,
        };

        client.ensure_default_apps()?;

        Ok(client)
    }

    fn create_info() -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(EXTENSION_NAME, "1.0.0").with_title("Apps Manager"))
        .with_instructions(
            "Use this extension to create, manage, and iterate on custom HTML/CSS/JavaScript apps.",
        )
    }

    fn ensure_default_apps(&self) -> Result<(), String> {
        // TODO(Douwe): we have the same check in cache, consider unifying that
        // Check if clock app exists
        let clock_path = self.apps_dir.join(format!("{CLOCK_APP_NAME}.html"));
        if !clock_path.exists() {
            // Parse and save the default clock app
            let clock_app = GooseApp::from_html(CLOCK_HTML)?;
            self.save_app(&clock_app)?;
        }

        Ok(())
    }

    fn list_stored_apps(&self) -> Result<Vec<String>, String> {
        let mut apps = Vec::new();

        let entries = fs::read_dir(&self.apps_dir)
            .map_err(|e| format!("Failed to read apps directory: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("html") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    apps.push(stem.to_string());
                }
            }
        }

        apps.sort();
        Ok(apps)
    }

    fn load_app(&self, name: &str) -> Result<GooseApp, String> {
        let path = self.app_path(name)?;
        if name == CLOCK_APP_NAME {
            return GooseApp::from_html(CLOCK_HTML);
        }

        let html =
            fs::read_to_string(&path).map_err(|e| format!("Failed to read app file: {}", e))?;

        GooseApp::from_html(&html)
    }

    fn load_editable_app(&self, name: &str) -> Result<GooseApp, String> {
        if name == CLOCK_APP_NAME {
            return Err(format!("Cannot modify bundled default app '{name}'"));
        }
        let app = self.load_app(name)?;
        if McpAppCache::is_bundled_default_uri(&app.resource.uri) {
            return Err(format!(
                "Cannot modify bundled default app '{}'",
                app.resource.name
            ));
        }
        Ok(app)
    }

    fn save_app(&self, app: &GooseApp) -> Result<(), String> {
        let path = self.app_path(&app.resource.name)?;

        let html_content = app.to_html()?;

        fs::write(&path, html_content).map_err(|e| format!("Failed to write app file: {}", e))?;

        Ok(())
    }

    fn delete_app(&self, name: &str) -> Result<(), String> {
        self.load_editable_app(name)?;
        let path = self.app_path(name)?;

        fs::remove_file(&path).map_err(|e| format!("Failed to delete app file: {}", e))?;

        Ok(())
    }

    fn app_path(&self, name: &str) -> Result<PathBuf, String> {
        if !is_single_file_name(name) {
            return Err("Invalid app name: expected a single file name".to_string());
        }

        Ok(self.apps_dir.join(format!("{name}.html")))
    }

    fn with_platform_notification(
        &self,
        result: CallToolResult,
        event_type: &str,
        app_name: &str,
    ) -> CallToolResult {
        let mut params = serde_json::Map::new();
        params.insert("app_name".to_string(), json!(app_name));
        self.context
            .result_with_platform_notification(result, EXTENSION_NAME, event_type, params)
    }

    async fn get_provider(&self) -> Result<Arc<dyn Provider>, String> {
        let extension_manager = self
            .context
            .extension_manager
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or("Extension manager not available")?;

        let provider_guard = extension_manager.get_provider().lock().await;

        let provider = provider_guard
            .as_ref()
            .ok_or("Provider not available")?
            .clone();

        Ok(provider)
    }

    async fn effective_model_config(
        &self,
        session_id: &str,
        provider_name: &str,
    ) -> Result<goose_providers::model::ModelConfig, String> {
        let model_config = self.context.model_config_for_session(session_id).await?;
        match crate::providers::get_from_registry(provider_name).await {
            Ok(entry) => Ok(entry
                .normalize_model_config(model_config.clone())
                .unwrap_or(model_config)),
            Err(_) => Ok(model_config),
        }
    }

    fn schema<T: JsonSchema>() -> JsonObject {
        serde_json::to_value(schema_for!(T))
            .map(|v| {
                v.as_object()
                    .expect("schema_for!(T) must serialize to a JSON object")
                    .clone()
            })
            .expect("Schema serialization must succeed")
    }

    fn create_app_content_tool() -> rmcp::model::Tool {
        rmcp::model::Tool::new(
            "create_app_content".to_string(),
            "Generate content for a new Goose app. Returns the HTML code, app name, description, and window properties.".to_string(),
            Self::schema::<CreateAppContentResponse>(),
        )
    }

    fn update_app_content_tool() -> rmcp::model::Tool {
        rmcp::model::Tool::new(
            "update_app_content".to_string(),
            "Generate updated content for an existing Goose app. Returns the improved HTML code, updated description, and optionally updated window properties.".to_string(),
            Self::schema::<UpdateAppContentResponse>(),
        )
    }

    async fn generate_new_app_content(
        &self,
        session_id: &str,
        prd: &str,
    ) -> Result<CreateAppContentResponse, String> {
        let provider = self.get_provider().await?;

        let existing_apps = self.list_stored_apps().unwrap_or_default();
        let existing_names = existing_apps.join(", ");

        let context: HashMap<&str, &str> = HashMap::new();
        let system_prompt = render_template("apps_create.md", &context)
            .map_err(|e| format!("Failed to render template: {}", e))?;

        let user_prompt = format!(
            "REQUESTED APP:\n{}\n\nEXISTING APPS: {}\n\nGenerate a unique name (lowercase with hyphens, not in existing apps), a brief description, complete HTML, and appropriate window size for this app.",
            prd,
            if existing_names.is_empty() { "none" } else { &existing_names }
        );

        let messages = vec![Message::user().with_text(&user_prompt)];
        let tools = vec![Self::create_app_content_tool()];

        let model_config = self
            .effective_model_config(session_id, provider.get_name())
            .await?;
        let (response, usage) = complete_app_content(
            provider,
            &model_config,
            session_id,
            system_prompt,
            &messages,
            tools,
        )
        .await?;

        if let (Some(output), Some(max)) = (usage.usage.output_tokens, model_config.max_tokens) {
            if output >= max {
                return Err("App content generation was truncated because the response hit the token limit. Try simplifying your app description.".to_string());
            }
        }

        extract_tool_response(
            &response,
            "create_app_content",
            &Self::schema::<CreateAppContentResponse>(),
        )
    }

    async fn generate_updated_app_content(
        &self,
        session_id: &str,
        existing_html: &str,
        existing_prd: &str,
        feedback: &str,
    ) -> Result<UpdateAppContentResponse, String> {
        let provider = self.get_provider().await?;

        let context: HashMap<&str, &str> = HashMap::new();
        let system_prompt = render_template("apps_iterate.md", &context)
            .map_err(|e| format!("Failed to render template: {}", e))?;

        let user_prompt = format!(
            "ORIGINAL PRD:\n{}\n\nCURRENT APP:\n```html\n{}\n```\n\nFEEDBACK: {}\n\nImplement the requested changes and return:\n1. Updated description\n2. Updated HTML implementing the feedback\n3. Updated PRD reflecting the current state of the app\n4. Optionally updated window size if appropriate",
            existing_prd,
            existing_html,
            feedback
        );

        let messages = vec![Message::user().with_text(&user_prompt)];
        let tools = vec![Self::update_app_content_tool()];

        let model_config = self
            .effective_model_config(session_id, provider.get_name())
            .await?;
        let (response, usage) = complete_app_content(
            provider,
            &model_config,
            session_id,
            system_prompt,
            &messages,
            tools,
        )
        .await?;

        if let (Some(output), Some(max)) = (usage.usage.output_tokens, model_config.max_tokens) {
            if output >= max {
                return Err("App content update was truncated because the response hit the token limit. Try requesting smaller changes.".to_string());
            }
        }

        extract_tool_response(
            &response,
            "update_app_content",
            &Self::schema::<UpdateAppContentResponse>(),
        )
    }

    async fn handle_list_apps(
        &self,
        _arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let app_names = self.list_stored_apps()?;

        if app_names.is_empty() {
            return Ok(CallToolResult::success(vec![ContentBlock::text(
                "No apps found. Create your first app with the create_app tool!".to_string(),
            )]));
        }

        let mut apps_info = vec![format!("Found {} app(s):\n", app_names.len())];

        for name in app_names {
            match self.load_app(&name) {
                Ok(app) => {
                    let description = app
                        .resource
                        .description
                        .as_deref()
                        .unwrap_or("No description");

                    let size = if let Some(ref props) = app.window_props {
                        format!(" ({}x{})", props.width, props.height)
                    } else {
                        String::new()
                    };

                    apps_info.push(format!("- {}{}: {}", name, size, description));
                }
                Err(e) => {
                    apps_info.push(format!("- {}: (error loading: {})", name, e));
                }
            }
        }

        Ok(CallToolResult::success(vec![ContentBlock::text(
            apps_info.join("\n"),
        )]))
    }

    async fn handle_create_app(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;
        let prd = extract_string(&args, "prd")?;

        let content = self.generate_new_app_content(session_id, &prd).await?;

        if self.load_app(&content.name).is_ok() {
            return Err(format!(
                "App '{}' already exists (generated name conflicts with existing app).",
                content.name
            ));
        }

        let app = GooseApp {
            resource: McpAppResource {
                uri: format!("ui://apps/{}", content.name),
                name: content.name.clone(),
                description: Some(content.description),
                mime_type: "text/html;profile=mcp-app".to_string(),
                text: Some(content.html),
                blob: None,
                meta: None,
            },
            mcp_servers: vec![EXTENSION_NAME.to_string()],
            window_props: Some(WindowProps {
                width: content.width.unwrap_or(DEFAULT_WINDOW_PROPS.width),
                height: content.height.unwrap_or(DEFAULT_WINDOW_PROPS.height),
                resizable: content.resizable.unwrap_or(DEFAULT_WINDOW_PROPS.resizable),
            }),
            prd: Some(prd),
            deletable: false,
        };

        self.save_app(&app)?;

        let result = CallToolResult::success(vec![ContentBlock::text(format!(
            "Created app '{}'! It should have automatically opened in a new window. You can always find it again in the [Apps] tab.",
            content.name
        ))]);

        Ok(self.with_platform_notification(result, "app_created", &content.name))
    }

    async fn handle_iterate_app(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;

        let name = extract_string(&args, "name")?;
        let feedback = extract_string(&args, "feedback")?;

        let mut app = self.load_editable_app(&name)?;

        let existing_html = app
            .resource
            .text
            .as_deref()
            .ok_or("App has no HTML content")?;

        let existing_prd = app.prd.as_deref().unwrap_or("");

        let content = self
            .generate_updated_app_content(session_id, existing_html, existing_prd, &feedback)
            .await?;

        app.resource.text = Some(content.html);
        app.resource.description = Some(content.description);
        app.prd = Some(content.prd);
        if content.width.is_some() || content.height.is_some() || content.resizable.is_some() {
            let current_props = app.window_props.as_ref();
            let default_width = current_props
                .map(|p| p.width)
                .unwrap_or(DEFAULT_WINDOW_PROPS.width);
            let default_height = current_props
                .map(|p| p.height)
                .unwrap_or(DEFAULT_WINDOW_PROPS.height);
            let default_resizable = current_props
                .map(|p| p.resizable)
                .unwrap_or(DEFAULT_WINDOW_PROPS.resizable);

            app.window_props = Some(WindowProps {
                width: content.width.unwrap_or(default_width),
                height: content.height.unwrap_or(default_height),
                resizable: content.resizable.unwrap_or(default_resizable),
            });
        }

        self.save_app(&app)?;

        let result = CallToolResult::success(vec![ContentBlock::text(format!(
            "Updated app '{}' based on your feedback",
            name
        ))]);

        Ok(self.with_platform_notification(result, "app_updated", &name))
    }

    async fn handle_delete_app(
        &self,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, String> {
        let args = arguments.ok_or("Missing arguments")?;

        let name = extract_string(&args, "name")?;

        self.delete_app(&name)?;

        let result =
            CallToolResult::success(vec![ContentBlock::text(format!("Deleted app '{}'", name))]);

        Ok(self.with_platform_notification(result, "app_deleted", &name))
    }
}

#[async_trait]
impl McpClientTrait for AppsManagerClient {
    async fn list_tools(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        let tools = vec![
            McpTool::new(
                "list_apps".to_string(),
                "List all available Goose apps with their names and descriptions. Use this to see what apps exist before creating or modifying apps.".to_string(),
                schema::<ListAppsParams>(),
            ),
            model_only_tool(McpTool::new(
                "create_app".to_string(),
                "Create a new Goose app based on a description or PRD. The extension will use an LLM to generate the HTML/CSS/JavaScript. Apps are sandboxed and run in standalone windows.".to_string(),
                schema::<CreateAppParams>(),
            )),
            model_only_tool(McpTool::new(
                "iterate_app".to_string(),
                "Improve an existing app based on feedback. The extension will use an LLM to update the HTML while preserving the app's intent.".to_string(),
                schema::<IterateAppParams>(),
            )),
            model_only_tool(McpTool::new(
                "delete_app".to_string(),
                "Delete an app permanently".to_string(),
                schema::<DeleteAppParams>(),
            )),
        ];

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
        _cancel_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let session_id = &ctx.session_id;
        let result = match name {
            "list_apps" => self.handle_list_apps(arguments).await,
            "create_app" => self.handle_create_app(session_id, arguments).await,
            "iterate_app" => self.handle_iterate_app(session_id, arguments).await,
            "delete_app" => self.handle_delete_app(arguments).await,
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

    async fn list_resources(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListResourcesResult, Error> {
        let app_names = self
            .list_stored_apps()
            .map_err(|_| Error::TransportClosed)?;

        let mut resources = Vec::new();

        for name in app_names {
            if let Ok(app) = self.load_app(&name) {
                let meta = if let Some(ref window_props) = app.window_props {
                    let mut meta_obj = MetaObject::new();
                    meta_obj.insert(
                        "window".to_string(),
                        json!({
                            "width": window_props.width,
                            "height": window_props.height,
                            "resizable": window_props.resizable,
                        }),
                    );
                    Some(meta_obj)
                } else {
                    None
                };

                let mut resource =
                    Resource::new(app.resource.uri.clone(), app.resource.name.clone())
                        .with_mime_type(app.resource.mime_type.clone());
                if let Some(description) = &app.resource.description {
                    resource = resource.with_description(description.clone());
                }
                resource.meta = meta;
                resources.push(resource);
            }
        }

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        _session_id: &str,
        uri: &str,
        _cancel_token: CancellationToken,
    ) -> Result<ReadResourceResult, Error> {
        let app_name = uri
            .strip_prefix("ui://apps/")
            .ok_or(Error::TransportClosed)?;

        let app = self
            .load_app(app_name)
            .map_err(|_| Error::TransportClosed)?;

        let html = app
            .resource
            .text
            .unwrap_or_else(|| String::from("No content"));

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            html, uri,
        )]))
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }
}

fn schema<T: JsonSchema>() -> JsonObject {
    let mut obj = serde_json::to_value(schema_for!(T))
        .map(|v| v.as_object().unwrap().clone())
        .expect("valid schema");
    // Ensure properties key exists (required by OpenAI-compatible APIs)
    obj.entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    obj
}

fn model_only_tool(tool: McpTool) -> McpTool {
    tool.with_meta(MetaObject(
        json!({ "ui": { "visibility": ["model"] } })
            .as_object()
            .unwrap()
            .clone(),
    ))
}

fn extract_string(args: &JsonObject, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Missing or invalid '{}'", key))
}

fn is_single_file_name(name: &str) -> bool {
    if name.contains(['/', '\\']) || has_windows_drive_prefix(name) {
        return false;
    }

    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn has_windows_drive_prefix(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn extract_tool_response<T: serde::de::DeserializeOwned>(
    response: &Message,
    tool_name: &str,
    tool_schema: &JsonObject,
) -> Result<T, String> {
    let schema_value = serde_json::Value::Object(tool_schema.clone());

    for content in &response.content {
        if let crate::conversation::message::MessageContent::ToolRequest(tool_req) = content {
            if let Ok(tool_call) = &tool_req.tool_call {
                if tool_call.name == tool_name {
                    let params = tool_call
                        .arguments
                        .as_ref()
                        .ok_or("Missing tool call parameters")?;

                    let coerced = coerce_tool_arguments(Some(params.clone()), &schema_value)
                        .unwrap_or_else(|| params.clone());

                    return serde_json::from_value(serde_json::Value::Object(coerced))
                        .map_err(|e| format!("Failed to parse tool response: {}", e));
                }
            }
        }
    }

    Err(format!("LLM did not call the required tool: {}", tool_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::reply_parts::{is_tool_visible_to_app, is_tool_visible_to_model};
    use crate::providers::base::{stream_from_single_message, MessageStream, Usage};
    use crate::session::SessionManager;
    use goose_providers::errors::ProviderError;
    use std::path::Path;
    use std::sync::Mutex;

    #[derive(Default)]
    struct AppContentProvider {
        request: Mutex<Option<(bool, String, usize)>>,
    }

    #[async_trait]
    impl Provider for AppContentProvider {
        fn get_name(&self) -> &str {
            "app-content-test"
        }

        async fn stream(
            &self,
            model_config: &goose_providers::model::ModelConfig,
            system: &str,
            _messages: &[Message],
            tools: &[McpTool],
        ) -> Result<MessageStream, ProviderError> {
            *self.request.lock().unwrap() =
                Some((model_config.toolshim, system.to_string(), tools.len()));
            let message = Message::assistant().with_text(
                r#"{"name":"create_app_content","arguments":{"name":"test-app","description":"Test app","html":"<html></html>"}}"#,
            );
            let usage = ProviderUsage::new("test-model".to_string(), Usage::default());
            Ok(stream_from_single_message(message, usage))
        }
    }

    fn test_client(apps_dir: PathBuf) -> AppsManagerClient {
        AppsManagerClient {
            info: AppsManagerClient::create_info(),
            context: PlatformExtensionContext {
                extension_manager: None,
                session_manager: Arc::new(SessionManager::new(apps_dir.join("sessions"))),
                scheduler: None,
                session: None,
                use_login_shell_path: false,
            },
            apps_dir,
        }
    }

    fn test_app(name: &str) -> GooseApp {
        GooseApp {
            resource: McpAppResource {
                uri: format!("ui://apps/{name}"),
                name: name.to_string(),
                description: Some("Test app".to_string()),
                mime_type: "text/html;profile=mcp-app".to_string(),
                text: Some("<html><body>test</body></html>".to_string()),
                blob: None,
                meta: None,
            },
            mcp_servers: vec![EXTENSION_NAME.to_string()],
            window_props: None,
            prd: None,
            deletable: true,
        }
    }

    #[tokio::test]
    async fn app_content_completion_uses_toolshim() {
        let provider = Arc::new(AppContentProvider::default());
        let model_config =
            goose_providers::model::ModelConfig::new("test-model").with_toolshim(true);
        let tool = AppsManagerClient::create_app_content_tool();
        let tool_schema = AppsManagerClient::schema::<CreateAppContentResponse>();

        let (response, _) = complete_app_content(
            provider.clone(),
            &model_config,
            "session",
            "Create an app".to_string(),
            &[Message::user().with_text("A test app")],
            vec![tool],
        )
        .await
        .unwrap();

        let request = provider.request.lock().unwrap().clone().unwrap();
        assert!(request.0);
        assert!(request.1.contains("Tool Name: create_app_content"));
        assert_eq!(request.2, 0);

        let content: CreateAppContentResponse =
            extract_tool_response(&response, "create_app_content", &tool_schema).unwrap();
        assert_eq!(content.name, "test-app");
    }

    fn invalid_app_names(temp_dir: &Path) -> Vec<String> {
        vec![
            "../outside".to_string(),
            "nested/outside".to_string(),
            "nested/".to_string(),
            "nested/.".to_string(),
            r"nested\outside".to_string(),
            temp_dir.join("absolute").to_string_lossy().into_owned(),
            "C:outside".to_string(),
        ]
    }

    fn assert_invalid_name(error: String) {
        assert!(
            error.contains("Invalid app name"),
            "expected app-name validation error, got: {error}"
        );
    }

    #[tokio::test]
    async fn management_tools_are_model_only_while_listing_remains_shared() {
        let temp = tempfile::tempdir().unwrap();
        let client = test_client(temp.path().join("apps"));
        let tools = client
            .list_tools("session", None, CancellationToken::new())
            .await
            .unwrap()
            .tools;

        for name in ["create_app", "iterate_app", "delete_app"] {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            let visibility = &tool.meta.as_ref().unwrap().0["ui"]["visibility"];

            assert_eq!(visibility, &json!(["model"]));
            assert!(!is_tool_visible_to_app(tool));
            assert!(is_tool_visible_to_model(tool));
        }

        let list_apps = tools.iter().find(|tool| tool.name == "list_apps").unwrap();
        assert!(list_apps.meta.is_none());
        assert!(is_tool_visible_to_app(list_apps));
        assert!(is_tool_visible_to_model(list_apps));
    }

    #[tokio::test]
    async fn model_visible_management_tool_still_dispatches() {
        let temp = tempfile::tempdir().unwrap();
        let apps_dir = temp.path().join("apps");
        fs::create_dir_all(&apps_dir).unwrap();
        let client = test_client(apps_dir);
        client.save_app(&test_app("legitimate-app")).unwrap();

        let result = client
            .call_tool(
                &ToolCallContext::new("session".to_string(), None, None),
                "delete_app",
                Some(
                    json!({ "name": "legitimate-app" })
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        assert!(!client.apps_dir.join("legitimate-app.html").exists());
    }

    #[tokio::test]
    async fn load_and_resource_read_reject_unsafe_app_names() {
        let temp = tempfile::tempdir().unwrap();
        let apps_dir = temp.path().join("apps");
        fs::create_dir_all(&apps_dir).unwrap();
        let client = test_client(apps_dir);
        fs::write(
            temp.path().join("outside.html"),
            test_app("outside").to_html().unwrap(),
        )
        .unwrap();

        for name in invalid_app_names(temp.path()) {
            assert_invalid_name(client.load_app(&name).unwrap_err());

            let result = client
                .read_resource(
                    "session",
                    &format!("ui://apps/{name}"),
                    CancellationToken::new(),
                )
                .await;
            assert!(
                result.is_err(),
                "resource read accepted unsafe name: {name}"
            );
        }

        client.save_app(&test_app("legitimate-app")).unwrap();
        assert!(client.load_app("legitimate-app").is_ok());
        assert!(client
            .read_resource(
                "session",
                "ui://apps/legitimate-app",
                CancellationToken::new(),
            )
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn provider_generated_save_rejects_unsafe_app_names() {
        let temp = tempfile::tempdir().unwrap();
        let apps_dir = temp.path().join("apps");
        fs::create_dir_all(&apps_dir).unwrap();
        let client = test_client(apps_dir);

        for name in invalid_app_names(temp.path()) {
            assert_invalid_name(client.save_app(&test_app(&name)).unwrap_err());
        }

        client.save_app(&test_app("legitimate-app")).unwrap();
        assert_eq!(
            client.load_app("legitimate-app").unwrap().resource.name,
            "legitimate-app"
        );
    }

    #[tokio::test]
    async fn bundled_default_apps_cannot_be_modified_or_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let apps_dir = temp.path().join("apps");
        fs::create_dir_all(&apps_dir).unwrap();
        let client = test_client(apps_dir);

        let forged_app = test_app("forged-clock");
        fs::write(
            client.apps_dir.join("clock.html"),
            forged_app.to_html().unwrap(),
        )
        .unwrap();
        let error = client.load_editable_app("clock").unwrap_err();
        assert_eq!(error, "Cannot modify bundled default app 'clock'");
        let error = client.delete_app("clock").unwrap_err();
        assert_eq!(error, "Cannot modify bundled default app 'clock'");
        assert!(client.apps_dir.join("clock.html").exists());
        let compiled_clock = GooseApp::from_html(CLOCK_HTML).unwrap();
        let loaded_clock = client.load_app("clock").unwrap();
        assert_eq!(loaded_clock.resource.name, compiled_clock.resource.name);
        assert_eq!(loaded_clock.resource.uri, compiled_clock.resource.uri);
        assert_eq!(loaded_clock.resource.text, compiled_clock.resource.text);
        let resource = client
            .read_resource("session", "ui://apps/clock", CancellationToken::new())
            .await
            .unwrap();
        let resource = serde_json::to_value(resource).unwrap();
        assert_eq!(
            resource["contents"][0]["text"].as_str(),
            compiled_clock.resource.text.as_deref()
        );

        client.save_app(&test_app("legitimate-app")).unwrap();
        assert!(client.load_editable_app("legitimate-app").is_ok());
        client.delete_app("legitimate-app").unwrap();
        assert!(!client.apps_dir.join("legitimate-app.html").exists());
    }

    #[tokio::test]
    async fn delete_rejects_unsafe_app_names() {
        let temp = tempfile::tempdir().unwrap();
        let apps_dir = temp.path().join("apps");
        fs::create_dir_all(&apps_dir).unwrap();
        let client = test_client(apps_dir);
        let outside_path = temp.path().join("outside.html");
        fs::write(&outside_path, "outside").unwrap();

        for name in invalid_app_names(temp.path()) {
            assert_invalid_name(client.delete_app(&name).unwrap_err());
        }

        assert!(outside_path.exists());
        client.save_app(&test_app("legitimate-app")).unwrap();
        client.delete_app("legitimate-app").unwrap();
        assert!(!client.apps_dir.join("legitimate-app.html").exists());
    }
}
