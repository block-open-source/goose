use crate::agents::extension::PlatformExtensionContext;
use crate::agents::extension_manager::{get_tool_owner, get_tool_resource_uri};
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::reply_parts::is_tool_visible_to_model;
use crate::agents::tool_execution::ToolCallContext;
use anyhow::Result;
use async_trait::async_trait;
use pctx_code_mode::{
    config::ToolDisclosure,
    descriptions::{tools as tool_descriptions, workflow::get_workflow_description},
    model::{CallbackConfig, ExecuteBashInput, ExecuteTypescriptInput, GetFunctionDetailsInput},
    registry::{CallbackFn, PctxRegistry},
    CodeMode,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, InitializeResult,
    JsonObject, ListToolsResult, Role, ServerCapabilities, Tool as McpTool, ToolAnnotations,
};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub static EXTENSION_NAME: &str = "code_execution";

pub struct CodeExecutionClient {
    info: InitializeResult,
    context: PlatformExtensionContext,
    disclosure: ToolDisclosure,
    state: RwLock<Option<CodeModeState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct ToolGraphNode {
    /// Tool name in format "server/tool" (e.g., "developer/shell")
    tool: String,
    /// Brief description of what this call does (e.g., "list files in /src")
    description: String,
    /// Indices of nodes this depends on (empty if no dependencies)
    #[serde(default)]
    depends_on: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExecuteWithToolGraph {
    #[serde(flatten)]
    input: ExecuteTypescriptInput,
    /// DAG of tool calls showing execution flow. Each node represents a tool call.
    /// Use depends_on to show data flow (e.g., node 1 uses output from node 0).
    #[serde(default)]
    tool_graph: Vec<ToolGraphNode>,
}

impl CodeExecutionClient {
    pub fn new(context: PlatformExtensionContext, disclosure: ToolDisclosure) -> Result<Self> {
        let info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(EXTENSION_NAME.to_string(), "1.0.0".to_string())
                    .with_title("Code Mode"),
            )
            .with_instructions(get_workflow_description(disclosure));

        Ok(Self {
            info,
            context,
            disclosure,
            state: RwLock::new(None),
        })
    }

    async fn load_callback_configs(&self, session_id: &str) -> Option<Vec<CallbackConfig>> {
        let manager = self
            .context
            .extension_manager
            .as_ref()
            .and_then(|w| w.upgrade())?;

        let tools = manager
            .get_prefixed_tools_excluding(session_id, EXTENSION_NAME)
            .await
            .ok()?;

        let mut cfgs = vec![];
        for tool in tools {
            if get_tool_resource_uri(&tool).is_some() || !is_tool_visible_to_model(&tool) {
                continue;
            }

            let (name, namespace) = if let Some((prefix, tool_name)) = tool.name.split_once("__") {
                (tool_name.to_string(), Some(prefix.to_string()))
            } else if let Some(owner) = get_tool_owner(&tool) {
                (tool.name.to_string(), Some(owner))
            } else {
                (tool.name.to_string(), None)
            };

            cfgs.push(CallbackConfig {
                name,
                namespace,
                description: tool.description.as_ref().map(|d| d.to_string()),
                input_schema: Some(json!(tool.input_schema)),
                output_schema: None,
            })
        }
        Some(cfgs)
    }

    /// Get the cached CodeMode, rebuilding if callback configs have changed
    async fn get_code_mode(&self, session_id: &str) -> Result<CodeMode, String> {
        let cfgs = self
            .load_callback_configs(session_id)
            .await
            .ok_or("Failed to load callback configs")?;
        let current_hash = CodeModeState::hash(&cfgs);

        // Use cache if no state change
        {
            let guard = self.state.read().await;
            if let Some(state) = guard.as_ref() {
                if state.hash == current_hash {
                    return Ok(state.code_mode.clone());
                }
            }
        }

        // Rebuild CodeMode & cache
        let mut guard = self.state.write().await;
        // Double-check after acquiring write lock
        if let Some(state) = guard.as_ref() {
            if state.hash == current_hash {
                return Ok(state.code_mode.clone());
            }
        }

        let state = CodeModeState::new(cfgs)?;
        let code_mode = state.code_mode.clone();
        *guard = Some(state);

        Ok(code_mode)
    }

    /// Build a PctxRegistry with all tool callbacks registered
    fn build_callback_registry(
        &self,
        ctx: &ToolCallContext,
        code_mode: &CodeMode,
        cancellation_token: CancellationToken,
        rt: tokio::runtime::Handle,
    ) -> Result<PctxRegistry, String> {
        let manager = self
            .context
            .extension_manager
            .as_ref()
            .and_then(|w| w.upgrade())
            .ok_or("Extension manager not available")?;

        let registry = PctxRegistry::default();
        for cfg in code_mode.callbacks() {
            let full_name = format!(
                "{}{}",
                cfg.namespace
                    .clone()
                    .map(|n| format!("{n}__"))
                    .unwrap_or_default(),
                cfg.name
            );
            let callback = create_tool_callback(
                ctx.clone(),
                full_name,
                manager.clone(),
                cancellation_token.clone(),
                rt.clone(),
            );
            registry
                .add_callback(&cfg.id(), callback)
                .map_err(|e| format!("Failed to register callback: {e}"))?;
        }

        Ok(registry)
    }

    /// Handle the list_functions tool call
    async fn handle_list_functions(&self, session_id: &str) -> Result<Vec<ContentBlock>, String> {
        let code_mode = self.get_code_mode(session_id).await?;
        let output = code_mode.list_functions();

        Ok(vec![ContentBlock::text(output.code)])
    }

    /// Handle the get_function_details tool call
    async fn handle_get_function_details(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
    ) -> Result<Vec<ContentBlock>, String> {
        let input: GetFunctionDetailsInput = arguments
            .map(|args| serde_json::from_value(Value::Object(args)))
            .transpose()
            .map_err(|e| format!("Failed to parse arguments: {e}"))?
            .ok_or("Missing arguments for get_function_details")?;

        let code_mode = self.get_code_mode(session_id).await?;
        let output = code_mode.get_function_details(input);

        Ok(vec![ContentBlock::text(output.code)])
    }

    /// Handle the execute bash tool call
    async fn handle_execute_bash(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<ContentBlock>, String> {
        let input: ExecuteBashInput = arguments
            .map(|args| serde_json::from_value(Value::Object(args)))
            .transpose()
            .map_err(|e| format!("Failed to parse arguments: {e}"))?
            .ok_or("Missing arguments for execute_bash")?;
        let command = input.command;
        let code_mode = self.get_code_mode(session_id).await?;

        let dispatch_token = cancellation_token.child_token();
        let output = run_in_deno_runtime(
            execution_timeout(),
            cancellation_token,
            dispatch_token,
            move || async move {
                code_mode
                    .execute_bash(&command)
                    .await
                    .map_err(|e| format!("Bash execution error: {e}"))
            },
        )
        .await?;

        Ok(vec![ContentBlock::text(format!(
            "Exit Code: {}\n\n# STDOUT\n{}\n\n# STDERR\n{}",
            output.exit_code, output.stdout, output.stderr
        ))])
    }

    /// Handle the execute typescript tool call
    async fn handle_execute_typescript(
        &self,
        ctx: &ToolCallContext,
        arguments: Option<JsonObject>,
        cancellation_token: CancellationToken,
    ) -> Result<Vec<ContentBlock>, String> {
        let args: ExecuteWithToolGraph = arguments
            .map(|args| serde_json::from_value(Value::Object(args)))
            .transpose()
            .map_err(|e| format!("Failed to parse arguments: {e}"))?
            .ok_or("Missing arguments for execute_typescript")?;

        let session_id = &ctx.session_id;
        let code_mode = self.get_code_mode(session_id).await?;
        let dispatch_token = cancellation_token.child_token();
        let rt = tokio::runtime::Handle::current();
        let registry = self.build_callback_registry(ctx, &code_mode, dispatch_token.clone(), rt)?;
        let code = args.input.code.clone();
        let disclosure = self.disclosure;

        let output = run_in_deno_runtime(
            execution_timeout(),
            cancellation_token,
            dispatch_token,
            move || async move {
                code_mode
                    .execute_typescript(&code, disclosure, Some(registry))
                    .await
                    .map_err(|e| format!("Typescript execution error: {e}"))
            },
        )
        .await?;

        Ok(vec![ContentBlock::text(output.markdown())])
    }
}

fn execution_timeout() -> Duration {
    let secs = crate::config::Config::global()
        .get_goose_default_extension_timeout()
        .unwrap_or(crate::config::DEFAULT_EXTENSION_TIMEOUT);
    Duration::from_secs(secs)
}

/// Deno runtime is not Send, so execution runs in a blocking task with its
/// own tokio runtime. pctx serializes all executions behind a process-wide
/// V8 mutex, so a hung script would wedge code execution for every session:
/// bound the wait with the extension timeout and honor cancellation.
///
/// `dispatch_token` is the child token shared with the callback dispatches that
/// a script makes back into Goose tools. When execution is abandoned (timeout
/// or cancellation), the token is cancelled so an in-flight nested tool call
/// (e.g. a long `developer.shell` command) is told to stop instead of running
/// on in the background.
/// Grace period for nested tool calls to observe a dispatched cancellation
/// signal and clean up (e.g. kill child processes) before the task future is
/// abandoned.
const DISPATCH_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

async fn run_in_deno_runtime<T, F, Fut>(
    timeout: Duration,
    cancellation_token: CancellationToken,
    dispatch_token: CancellationToken,
    task: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, String>>,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create runtime: {e}"))?;

        rt.block_on(async move {
            let task_future = task();
            tokio::pin!(task_future);

            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    dispatch_token.cancel();
                    let _ = tokio::time::timeout(
                        DISPATCH_DRAIN_TIMEOUT,
                        &mut task_future,
                    ).await;
                    Err("Execution cancelled".to_string())
                }
                _ = tokio::time::sleep(timeout) => {
                    dispatch_token.cancel();
                    let _ = tokio::time::timeout(
                        DISPATCH_DRAIN_TIMEOUT,
                        &mut task_future,
                    ).await;
                    Err(format!(
                        "Execution timed out after {} seconds",
                        timeout.as_secs()
                    ))
                }
                result = &mut task_future => result,
            }
        })
    })
    .await
    .map_err(|e| format!("Execution task failed: {e}"))?
}

fn create_tool_callback(
    ctx: ToolCallContext,
    full_name: String,
    manager: Arc<crate::agents::ExtensionManager>,
    cancellation_token: CancellationToken,
    rt: tokio::runtime::Handle,
) -> CallbackFn {
    Arc::new(move |args: Option<Value>| {
        let ctx = ctx.clone();
        let full_name = full_name.clone();
        let manager = manager.clone();
        let cancellation_token = cancellation_token.clone();
        let rt = rt.clone();
        Box::pin(async move {
            let tool_call = {
                let mut params = CallToolRequestParams::new(full_name);
                if let Some(args) = args.and_then(|v| v.as_object().cloned()) {
                    params = params.with_arguments(args);
                }
                params
            };

            let handle = rt.spawn(async move {
                match manager
                    .dispatch_tool_call(&ctx, tool_call, cancellation_token)
                    .await
                {
                    Ok(dispatch_result) => match dispatch_result.result.await {
                        Ok(result) => Ok(callback_result_to_value(&result)),
                        Err(e) => Err(format!("Tool error: {}", e.message)),
                    },
                    Err(e) => Err(format!("Dispatch error: {e}")),
                }
            });

            struct AbortOnDrop(tokio::task::AbortHandle);
            impl Drop for AbortOnDrop {
                fn drop(&mut self) {
                    self.0.abort();
                }
            }
            let _guard = AbortOnDrop(handle.abort_handle());
            handle
                .await
                .unwrap_or_else(|e| Err(format!("Callback task failed: {e}")))
        }) as Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>
    })
}

fn callback_result_to_value(result: &CallToolResult) -> Value {
    let text = result
        .content
        .iter()
        .filter_map(|content| match content {
            ContentBlock::Text(text)
                if text
                    .annotations
                    .as_ref()
                    .and_then(|annotations| annotations.audience.as_ref())
                    .is_none_or(|audiences| {
                        audiences.is_empty() || audiences.contains(&Role::Assistant)
                    }) =>
            {
                Some(text.text.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

#[async_trait]
impl McpClientTrait for CodeExecutionClient {
    #[allow(clippy::too_many_lines)]
    async fn list_tools(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        fn schema<T: JsonSchema>() -> JsonObject {
            serde_json::to_value(schema_for!(T))
                .map(|v| v.as_object().unwrap().clone())
                .expect("valid schema")
        }

        // Empty schema for list_functions (no parameters)
        let empty_schema: JsonObject = serde_json::from_value(json!({
            "type": "object",
            "properties": {},
            "required": []
        }))
        .expect("valid schema");

        let tools = match self.disclosure {
            ToolDisclosure::Catalog => {
                vec![
                    McpTool::new(
                        "list_functions".to_string(),
                        tool_descriptions::LIST_FUNCTIONS.to_string(),
                        empty_schema,
                    )
                    .annotate(ToolAnnotations::from_raw(
                        Some("List functions".to_string()),
                        Some(true),
                        Some(false),
                        Some(true),
                        Some(false),
                    )),
                    McpTool::new(
                        "get_function_details".to_string(),
                        tool_descriptions::GET_FUNCTION_DETAILS.to_string(),
                        schema::<GetFunctionDetailsInput>(),
                    )
                    .annotate(ToolAnnotations::from_raw(
                        Some("Get function details".to_string()),
                        Some(true),
                        Some(false),
                        Some(true),
                        Some(false),
                    )),
                    McpTool::new(
                        "execute_typescript".to_string(),
                        tool_descriptions::EXECUTE_TYPESCRIPT_CATALOG.to_string(),
                        schema::<ExecuteWithToolGraph>(),
                    )
                    .annotate(ToolAnnotations::from_raw(
                        Some("Execute TypeScript".to_string()),
                        Some(false),
                        Some(true),
                        Some(false),
                        Some(true),
                    )),
                ]
            }
            ToolDisclosure::Filesystem => {
                vec![
                    McpTool::new(
                        "execute_bash".to_string(),
                        tool_descriptions::EXECUTE_BASH.to_string(),
                        schema::<ExecuteBashInput>(),
                    )
                    .annotate(ToolAnnotations::from_raw(
                        Some("Execute Bash".to_string()),
                        Some(false),
                        Some(true),
                        Some(false),
                        Some(true),
                    )),
                    McpTool::new(
                        "execute_typescript".to_string(),
                        tool_descriptions::EXECUTE_TYPESCRIPT_FILESYSTEM.to_string(),
                        schema::<ExecuteWithToolGraph>(),
                    )
                    .annotate(ToolAnnotations::from_raw(
                        Some("Execute TypeScript".to_string()),
                        Some(false),
                        Some(true),
                        Some(false),
                        Some(true),
                    )),
                ]
            }
            ToolDisclosure::Sidecar => {
                vec![McpTool::new(
                    "execute_typescript".to_string(),
                    tool_descriptions::EXECUTE_TYPESCRIPT_SIDECAR.to_string(),
                    schema::<ExecuteWithToolGraph>(),
                )
                .annotate(ToolAnnotations::from_raw(
                    Some("Execute TypeScript".to_string()),
                    Some(false),
                    Some(true),
                    Some(false),
                    Some(true),
                ))]
            }
        };

        Ok(ListToolsResult {
            meta: None,
            next_cursor: None,
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancellation_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let session_id = &ctx.session_id;
        let result = match name {
            "list_functions" => self.handle_list_functions(session_id).await,
            "get_function_details" => {
                self.handle_get_function_details(session_id, arguments)
                    .await
            }
            "execute_bash" => {
                self.handle_execute_bash(session_id, arguments, cancellation_token)
                    .await
            }
            "execute_typescript" => {
                self.handle_execute_typescript(ctx, arguments, cancellation_token)
                    .await
            }
            _ => Err(format!("Unknown tool: {name}")),
        };

        match result {
            Ok(content) => Ok(CallToolResult::success(content)),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Error: {error}"
            ))])),
        }
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }

    async fn get_moim(&self, session_id: &str) -> Option<String> {
        let code_mode = self.get_code_mode(session_id).await.ok()?;

        let disclosure_style_moim = match self.disclosure {
            ToolDisclosure::Catalog => {
                let function_count = code_mode.list_functions().functions.len();
                catalog_disclosure_moim(function_count)
            }
            ToolDisclosure::Filesystem => {
                let available_filepaths: Vec<_> = code_mode
                    .virtual_fs().keys().map(String::from).collect();
                format!("Use execute_bash to search and read the tool signatures and input/output types before calling execute_typescript. The available files are: {}", available_filepaths.join(", "))
            },
            ToolDisclosure::Sidecar => "Prioritize calling tools with the execute_typescript tool, especially when multiple tools can be called in one script.".into(),
        };

        Some(format!(
            indoc::indoc! {r#"
                ALWAYS batch multiple tool operations into ONE execute_typescript call.
                - WRONG: Separate execute_typescript calls for read file, then write file
                - RIGHT: One execute_typescript with an async run() function that reads AND writes AND logs/returns as little information as needed for the next step.

                {}
            "#},
            disclosure_style_moim
        ))
    }
}

fn catalog_disclosure_moim(function_count: usize) -> String {
    if function_count == 0 {
        "No execute_typescript callback functions are currently registered.".to_string()
    } else {
        format!(
            "{function_count} callback functions are available only from inside execute_typescript. Do not call callback function names directly as tools. Use list_functions and get_function_details to inspect signatures before writing one execute_typescript call."
        )
    }
}

pub fn get_tool_disclosure() -> ToolDisclosure {
    let config = crate::config::Config::global();
    let tool_disclosure_str: String = config
        .get_param("CODE_MODE_TOOL_DISCLOSURE")
        .unwrap_or_else(|_| "catalog".to_string());
    serde_json::from_value(serde_json::json!(tool_disclosure_str)).unwrap_or_default()
}

struct CodeModeState {
    code_mode: CodeMode,
    hash: u64,
}

impl CodeModeState {
    fn new(cfgs: Vec<CallbackConfig>) -> Result<Self, String> {
        let hash = Self::hash(&cfgs);

        let (code_mode, report) = CodeMode::default().with_callbacks(&cfgs);
        if !report.failed.is_empty() {
            let failures = report
                .failed
                .iter()
                .map(|failure| format!("{}: {}", failure.id, failure.reason))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "failed adding callback configs to CodeMode: {failures}"
            ));
        }

        Ok(Self { code_mode, hash })
    }

    /// Compute order-independent hash of callback configs
    fn hash(cfgs: &[CallbackConfig]) -> u64 {
        let mut cfg_strings: Vec<_> = cfgs
            .iter()
            .filter_map(|c| serde_json::to_string(c).ok())
            .collect();
        cfg_strings.sort();

        let mut hasher = DefaultHasher::new();
        for s in cfg_strings {
            s.hash(&mut hasher);
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::extension::ExtensionConfig;
    use crate::agents::extension_manager::ExtensionManager;
    use pctx_code_mode::model::FunctionId;
    use rmcp::model::{Annotations, EmbeddedResource, MetaObject, ResourceContents, TextContent};

    #[test]
    fn callback_result_ignores_hidden_structured_content_and_meta() {
        let mut result = CallToolResult::success(vec![ContentBlock::text("visible text")]);
        result.structured_content = Some(json!({"hidden": "structured secret"}));
        result.meta = Some(MetaObject(
            json!({"hidden": "metadata secret"})
                .as_object()
                .unwrap()
                .clone(),
        ));

        assert_eq!(
            callback_result_to_value(&result),
            Value::String("visible text".to_string())
        );
    }

    #[test]
    fn callback_result_uses_assistant_visible_text_and_preserves_json_parsing() {
        let user_only = ContentBlock::Text(
            TextContent::new(r#"false,"hidden":"user secret","ignored":"#)
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let assistant_only = ContentBlock::Text(
            TextContent::new("true}")
                .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
        );
        let resource = ResourceContents::TextResourceContents {
            uri: "file:///hidden.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "resource secret".to_string(),
            meta: None,
        };
        let result = CallToolResult::success(vec![
            ContentBlock::text(r#"{"visible":"#),
            user_only,
            ContentBlock::image("image secret", "image/png"),
            ContentBlock::Resource(EmbeddedResource::new(resource)),
            assistant_only,
        ]);

        assert_eq!(callback_result_to_value(&result), json!({"visible": true}));
    }

    struct VisibilityClient;

    #[async_trait]
    impl McpClientTrait for VisibilityClient {
        async fn list_tools(
            &self,
            _session_id: &str,
            _next_cursor: Option<String>,
            _cancellation_token: CancellationToken,
        ) -> Result<ListToolsResult, Error> {
            let app_only = McpTool::new(
                "app_only".to_string(),
                "App-only tool".to_string(),
                JsonObject::new(),
            )
            .with_meta(MetaObject(
                json!({ "ui": { "visibility": ["app"] } })
                    .as_object()
                    .unwrap()
                    .clone(),
            ));
            let model_visible = McpTool::new(
                "model_visible".to_string(),
                "Model-visible tool".to_string(),
                JsonObject::new(),
            )
            .with_output_schema::<ToolGraphNode>()
            .with_meta(MetaObject(
                json!({ "ui": { "visibility": ["model"] } })
                    .as_object()
                    .unwrap()
                    .clone(),
            ));
            let ordinary = McpTool::new(
                "ordinary".to_string(),
                "Ordinary tool".to_string(),
                JsonObject::new(),
            );

            Ok(ListToolsResult {
                tools: vec![app_only, model_visible, ordinary],
                ..Default::default()
            })
        }

        async fn call_tool(
            &self,
            _ctx: &ToolCallContext,
            _name: &str,
            _arguments: Option<JsonObject>,
            _cancellation_token: CancellationToken,
        ) -> Result<CallToolResult, Error> {
            Err(Error::TransportClosed)
        }

        fn get_info(&self) -> Option<&InitializeResult> {
            None
        }
    }

    #[tokio::test]
    async fn callback_configs_exclude_tools_hidden_from_model() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(ExtensionManager::new_without_provider(
            temp.path().join("manager"),
        ));
        manager
            .add_client(
                "visibility".to_string(),
                ExtensionConfig::Builtin {
                    name: "visibility".to_string(),
                    description: "Visibility test tools".to_string(),
                    display_name: None,
                    timeout: None,
                    bundled: None,
                    available_tools: vec![],
                },
                Arc::new(VisibilityClient),
                None,
            )
            .await;

        let mut context = manager.get_context().clone();
        context.extension_manager = Some(Arc::downgrade(&manager));
        let client = CodeExecutionClient::new(context, ToolDisclosure::Catalog).unwrap();
        let configs = client.load_callback_configs("test-session").await.unwrap();
        let names = configs
            .iter()
            .map(|config| config.name.as_str())
            .collect::<Vec<_>>();

        assert!(!names.contains(&"app_only"));
        assert!(names.contains(&"model_visible"));
        assert!(names.contains(&"ordinary"));
        assert!(configs.iter().all(|config| config.output_schema.is_none()));
    }

    #[tokio::test]
    async fn run_in_deno_runtime_times_out_on_hung_execution() {
        let result: Result<(), String> = run_in_deno_runtime(
            Duration::from_millis(50),
            CancellationToken::new(),
            CancellationToken::new(),
            std::future::pending,
        )
        .await;

        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn run_in_deno_runtime_honors_cancellation() {
        let token = CancellationToken::new();
        token.cancel();

        let result: Result<(), String> = run_in_deno_runtime(
            Duration::from_secs(60),
            token,
            CancellationToken::new(),
            std::future::pending,
        )
        .await;

        assert_eq!(result.unwrap_err(), "Execution cancelled");
    }

    #[tokio::test]
    async fn run_in_deno_runtime_cancels_dispatch_token_when_abandoned() {
        // On timeout, an in-flight nested tool call (via the dispatch token)
        // must be told to stop rather than left running in the background.
        let dispatch_token = CancellationToken::new();
        let result: Result<(), String> = run_in_deno_runtime(
            Duration::from_millis(50),
            CancellationToken::new(),
            dispatch_token.clone(),
            std::future::pending,
        )
        .await;
        assert!(result.unwrap_err().contains("timed out"));
        assert!(dispatch_token.is_cancelled());

        // On cancellation, the child dispatch token is likewise cancelled.
        let outer = CancellationToken::new();
        outer.cancel();
        let dispatch_token = outer.child_token();
        let result: Result<(), String> = run_in_deno_runtime(
            Duration::from_secs(60),
            outer,
            dispatch_token.clone(),
            std::future::pending,
        )
        .await;
        assert_eq!(result.unwrap_err(), "Execution cancelled");
        assert!(dispatch_token.is_cancelled());
    }

    #[tokio::test]
    async fn run_in_deno_runtime_drains_task_on_timeout() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dispatch_token = CancellationToken::new();
        let observed = Arc::new(AtomicBool::new(false));
        let task_token = dispatch_token.clone();
        let task_observed = observed.clone();

        let result: Result<(), String> = run_in_deno_runtime(
            Duration::from_millis(50),
            CancellationToken::new(),
            dispatch_token.clone(),
            move || async move {
                task_token.cancelled().await;
                task_observed.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await;

        assert!(
            result.unwrap_err().contains("timed out"),
            "should report timeout"
        );
        assert!(
            observed.load(Ordering::SeqCst),
            "task should observe dispatch token cancellation before being dropped"
        );
    }

    /// Exercises the real Deno/V8 stack: a script whose event loop never
    /// resolves must time out instead of wedging forever, and a normal
    /// script must run right after, proving pctx's process-wide V8 mutex
    /// was released (i.e. one hung execution no longer blocks other sessions).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_v8_hung_script_times_out_and_frees_the_runtime() {
        let hung = CodeMode::default();
        let hung_result = run_in_deno_runtime(
            Duration::from_secs(2),
            CancellationToken::new(),
            CancellationToken::new(),
            move || async move {
                hung.execute_typescript(
                    "async function run() { await new Promise(() => {}); }",
                    ToolDisclosure::default(),
                    None,
                )
                .await
                .map_err(|e| format!("execution error: {e}"))
            },
        )
        .await;
        assert!(
            hung_result.unwrap_err().contains("timed out"),
            "hung script should time out"
        );

        let normal = CodeMode::default();
        let normal_result = run_in_deno_runtime(
            Duration::from_secs(60),
            CancellationToken::new(),
            CancellationToken::new(),
            move || async move {
                normal
                    .execute_typescript(
                        "async function run() { return 1 + 1; }",
                        ToolDisclosure::default(),
                        None,
                    )
                    .await
                    .map_err(|e| format!("execution error: {e}"))
            },
        )
        .await
        .expect("normal script should run after a prior timeout");
        assert!(
            normal_result.success,
            "normal script should succeed once the V8 mutex is released: {}",
            normal_result.stderr
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn callback_completing_from_main_runtime_does_not_hang() {
        let rt = tokio::runtime::Handle::current();
        let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
        let rx = Arc::new(std::sync::Mutex::new(Some(rx)));

        let callback: CallbackFn = Arc::new({
            let rt = rt.clone();
            move |_args: Option<Value>| {
                let rt = rt.clone();
                let rx = rx.clone();
                Box::pin(async move {
                    let receiver = rx.lock().unwrap().take().expect("receiver taken once");
                    let handle = rt.spawn(async move {
                        receiver.await.map_err(|_| "channel closed".to_string())
                    });
                    handle.await.unwrap_or_else(|e| Err(e.to_string()))
                }) as Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>
            }
        });

        rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send(serde_json::json!({"done": true}));
        });

        let cfg = CallbackConfig {
            name: "ping".to_string(),
            namespace: Some("Test".to_string()),
            description: Some("ping".to_string()),
            input_schema: None,
            output_schema: None,
        };
        let code_mode = CodeMode::default()
            .with_callback(&cfg)
            .expect("add callback");
        let registry = PctxRegistry::default();
        registry.add_callback(&cfg.id(), callback).unwrap();

        let result = run_in_deno_runtime(
            Duration::from_secs(5),
            CancellationToken::new(),
            CancellationToken::new(),
            move || async move {
                code_mode
                    .execute_typescript(
                        "async function run() { return await Test.ping(); }",
                        ToolDisclosure::default(),
                        Some(registry),
                    )
                    .await
                    .map_err(|e| format!("execution error: {e}"))
            },
        )
        .await
        .expect("script should not time out");

        assert!(result.success, "callback should succeed: {}", result.stderr);
    }

    #[test]
    fn catalog_moim_mentions_inspection_tools_without_function_names() {
        let moim = catalog_disclosure_moim(3);

        assert!(moim.contains("3 callback functions"));
        assert!(moim.contains("list_functions"));
        assert!(moim.contains("get_function_details"));
        assert!(!moim.contains("extract_relations"));
        assert!(!moim.contains("ask_heimdall"));
    }

    #[tokio::test]
    async fn execute_bash_annotations_require_approval() {
        let temp = tempfile::tempdir().unwrap();
        let client = CodeExecutionClient::new(
            PlatformExtensionContext {
                extension_manager: None,
                session_manager: Arc::new(crate::session::SessionManager::new(
                    temp.path().join("sessions"),
                )),
                scheduler: None,
                session: None,
                use_login_shell_path: false,
            },
            ToolDisclosure::Filesystem,
        )
        .unwrap();

        let tools = client
            .list_tools("test", None, CancellationToken::new())
            .await
            .unwrap()
            .tools;
        let execute_bash = tools
            .iter()
            .find(|tool| tool.name == "execute_bash")
            .unwrap();
        let annotations = execute_bash.annotations.as_ref().unwrap();

        assert_eq!(annotations.title.as_deref(), Some("Execute Bash"));
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(false));
        assert_eq!(annotations.open_world_hint, Some(true));
    }

    fn self_referential_any_schema() -> Value {
        json!({
            "$ref": "#/$defs/Any",
            "$defs": {
                "Any": {
                    "anyOf": [
                        {"type": "string"},
                        {"type": "number"},
                        {
                            "type": "object",
                            "additionalProperties": {"$ref": "#/$defs/Any"}
                        }
                    ]
                }
            }
        })
    }

    #[test]
    fn code_mode_preserves_types_for_self_referential_schema() {
        let cfg = CallbackConfig {
            name: "retain".to_string(),
            namespace: Some("hindsight".to_string()),
            description: Some("Store a memory".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {"content": {"type": "string"}},
                "required": ["content"]
            })),
            output_schema: Some(self_referential_any_schema()),
        };

        let code_mode = CodeMode::default()
            .with_callback(&cfg)
            .expect("recursive schemas should be supported");
        let details = code_mode.get_function_details(GetFunctionDetailsInput {
            functions: vec![FunctionId {
                mod_name: "Hindsight".to_string(),
                fn_name: "retain".to_string(),
            }],
        });
        let function = details
            .functions
            .first()
            .expect("hindsight.retain should have generated details");

        assert_ne!(function.output_type, "any");
        assert!(
            function.types.contains("export type RetainOutputAny =")
                && function.types.contains("[key: string]: RetainOutputAny"),
            "expected RetainOutputAny to reference itself, got: {}",
            function.types
        );
    }
}
