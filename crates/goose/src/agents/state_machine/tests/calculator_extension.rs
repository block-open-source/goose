use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rmcp::model::{
    Annotations, CallToolResult, ContentBlock, ErrorData, GetPromptResult, Implementation,
    InitializeResult, JsonObject, ListPromptsResult, ListToolsResult, Prompt, PromptMessage, Role,
    ServerCapabilities, ServerNotification, Tool,
};
use schemars::{schema_for, JsonSchema};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::action_required_manager::{ActionRequiredManager, ElicitationOutcome};
use crate::agents::mcp_client::{Error as McpError, McpClientTrait};
use crate::agents::tool_execution::ToolCallContext;

pub(super) const ADD: &str = "calculator__add";
pub(super) const APP_ONLY: &str = "calculator__app_only";
pub(super) const ADD_VALUES: &str = "calculator__add_values";
pub(super) const ADD_WITH_AUDIENCE: &str = "calculator__add_with_audience";
pub(super) const DIVIDE: &str = "calculator__divide";
pub(super) const REQUEST_VALUE: &str = "calculator__request_value";
const EXECUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

pub(super) fn value(value: i64) -> Value {
    json!({ "value": value })
}

pub(super) fn delayed_value(value: i64, delay_ms: u64) -> Value {
    json!({ "value": value, "delay_ms": delay_ms })
}

pub(super) fn named_values<const N: usize>(values: [(&str, i64); N]) -> Value {
    Value::Object(
        values
            .into_iter()
            .map(|(name, value)| (name.to_string(), Value::from(value)))
            .collect(),
    )
}

#[derive(Deserialize, JsonSchema)]
struct ValueParams {
    value: i64,
    #[serde(default)]
    delay_ms: u64,
}

#[derive(JsonSchema)]
struct RequestValueParams {}

pub(super) struct CalculatorExtension {
    info: InitializeResult,
    action_required: Arc<ActionRequiredManager>,
    total: Mutex<i64>,
    contexts: Mutex<Vec<ToolCallContext>>,
    barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    completed: tokio::sync::Notify,
    notification_senders: Mutex<Vec<mpsc::Sender<ServerNotification>>>,
}

impl CalculatorExtension {
    pub(super) fn new(action_required: Arc<ActionRequiredManager>) -> Self {
        Self {
            info: InitializeResult::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_prompts()
                    .build(),
            )
            .with_server_info(Implementation::new(
                "calculator".to_string(),
                "test".to_string(),
            )),
            action_required,
            total: Mutex::new(0),
            contexts: Mutex::new(Vec::new()),
            barrier: Mutex::new(None),
            completed: tokio::sync::Notify::new(),
            notification_senders: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn total(&self) -> i64 {
        *self.total.lock().unwrap()
    }

    pub(super) fn synchronize(&self, calls: usize) {
        *self.barrier.lock().unwrap() = Some(Arc::new(tokio::sync::Barrier::new(calls)));
    }

    pub(super) fn contexts(&self) -> Vec<ToolCallContext> {
        self.contexts.lock().unwrap().clone()
    }

    pub(super) async fn wait_for_result(&self) {
        self.completed.notified().await;
    }
}

#[async_trait]
impl McpClientTrait for CalculatorExtension {
    async fn list_tools(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListToolsResult, McpError> {
        let schema = serde_json::to_value(schema_for!(ValueParams))
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        let request_value_schema = serde_json::to_value(schema_for!(RequestValueParams))
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        let add_values_schema = serde_json::to_value(schema_for!(HashMap<String, i64>))
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new(
                "add",
                "Add a value to the running total",
                Arc::new(schema.clone()),
            ),
            Tool::new(
                "app_only",
                "Add a value from an app",
                Arc::new(schema.clone()),
            )
            .with_meta(rmcp::model::MetaObject(
                json!({ "ui": { "visibility": ["app"] } })
                    .as_object()
                    .unwrap()
                    .clone(),
            )),
            Tool::new(
                "add_values",
                "Add named values to the running total",
                Arc::new(add_values_schema),
            ),
            Tool::new(
                "add_with_audience",
                "Add a value and return output for both the agent and user",
                Arc::new(schema.clone()),
            ),
            Tool::new(
                "divide",
                "Divide the running total by a value",
                Arc::new(schema.clone()),
            ),
            Tool::new(
                "multiply",
                "Multiply the running total by a value",
                Arc::new(schema.clone()),
            ),
            Tool::new(
                "subtract",
                "Subtract a value from the running total",
                Arc::new(schema),
            ),
            Tool::new(
                "request_value",
                "Ask the user for a value",
                Arc::new(request_value_schema),
            ),
        ]))
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancel_token: CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        self.contexts.lock().unwrap().push(ctx.clone());
        if name == "request_value" {
            let tool_call_request_id = ctx.tool_call_request_id.clone().ok_or_else(|| {
                McpError::McpError(ErrorData::invalid_params(
                    "request_value requires a tool call id",
                    None,
                ))
            })?;
            let schema = serde_json::to_value(schema_for!(ValueParams)).unwrap();
            let outcome = self
                .action_required
                .request_and_wait(
                    ctx.session_id.clone(),
                    tool_call_request_id,
                    "What value should I use?".to_string(),
                    schema,
                    std::time::Duration::from_secs(10),
                )
                .await
                .map_err(|error| {
                    McpError::McpError(ErrorData::internal_error(error.to_string(), None))
                })?;
            return Ok(match outcome {
                ElicitationOutcome::Accept(value) => {
                    let params: ValueParams = serde_json::from_value(value).map_err(|error| {
                        McpError::McpError(ErrorData::invalid_params(error.to_string(), None))
                    })?;
                    *self.total.lock().unwrap() = params.value;
                    CallToolResult::success(vec![ContentBlock::text(format!(
                        "result: {}",
                        params.value
                    ))])
                }
                ElicitationOutcome::Decline => {
                    CallToolResult::error(vec![ContentBlock::text("elicitation declined")])
                }
                ElicitationOutcome::Cancel => {
                    CallToolResult::error(vec![ContentBlock::text("elicitation cancelled")])
                }
            });
        }
        let arguments = Value::Object(arguments.unwrap_or_default());
        let (calculate, value, delay_ms): (fn(i64, i64) -> Option<i64>, i64, u64) =
            if name == "add_values" {
                let values: HashMap<String, i64> =
                    serde_json::from_value(arguments).map_err(|error| {
                        McpError::McpError(ErrorData::invalid_params(error.to_string(), None))
                    })?;
                let value = values
                    .into_values()
                    .try_fold(0_i64, i64::checked_add)
                    .ok_or_else(|| {
                        McpError::McpError(ErrorData::invalid_params(
                            "calculator operation failed",
                            None,
                        ))
                    })?;
                (i64::checked_add, value, 0)
            } else {
                let calculate = match name {
                    "add" | "add_with_audience" | "app_only" => i64::checked_add,
                    "divide" => i64::checked_div,
                    "multiply" => i64::checked_mul,
                    "subtract" => i64::checked_sub,
                    _ => {
                        return Err(McpError::McpError(ErrorData::invalid_params(
                            format!("unknown calculator tool {name:?}"),
                            None,
                        )));
                    }
                };
                let params: ValueParams = serde_json::from_value(arguments).map_err(|error| {
                    McpError::McpError(ErrorData::invalid_params(error.to_string(), None))
                })?;
                (calculate, params.value, params.delay_ms)
            };
        let barrier = self.barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            let result = tokio::select! {
                _ = cancel_token.cancelled() => {
                    return Err(McpError::McpError(ErrorData::internal_error(
                        "calculator call cancelled",
                        None,
                    )));
                }
                result = tokio::time::timeout(std::time::Duration::from_secs(2), barrier.wait()) => {
                    result.map_err(|_| {
                        McpError::McpError(ErrorData::internal_error(
                            "calculator calls did not execute concurrently",
                            None,
                        ))
                    })?
                }
            };
            if result.is_leader() {
                *self.barrier.lock().unwrap() = None;
            }
        }
        if delay_ms > 0 {
            let delay = std::time::Duration::from_millis(delay_ms);
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    return Err(McpError::McpError(ErrorData::internal_error(
                        "calculator call cancelled",
                        None,
                    )));
                }
                _ = tokio::time::sleep(delay.min(EXECUTION_TIMEOUT)) => {}
            }
            if delay > EXECUTION_TIMEOUT {
                return Err(McpError::McpError(ErrorData::internal_error(
                    "calculator operation timed out",
                    None,
                )));
            }
        }
        let mut total = self.total.lock().unwrap();
        *total = calculate(*total, value).ok_or_else(|| {
            McpError::McpError(ErrorData::invalid_params(
                "calculator operation failed",
                None,
            ))
        })?;
        let result = format!("result: {}", *total);
        self.completed.notify_one();
        if name == "add_with_audience" {
            return Ok(CallToolResult::success(vec![
                ContentBlock::Text(
                    rmcp::model::TextContent::new(result.clone()).with_annotations(
                        Annotations::default().with_audience(vec![Role::Assistant]),
                    ),
                ),
                ContentBlock::Text(
                    rmcp::model::TextContent::new(result)
                        .with_annotations(Annotations::default().with_audience(vec![Role::User])),
                ),
            ]));
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(result)]))
    }

    async fn list_prompts(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancel_token: CancellationToken,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "explain_addition",
            Some("Explain an earlier answer"),
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        _session_id: &str,
        name: &str,
        _arguments: Value,
        _cancel_token: CancellationToken,
    ) -> Result<GetPromptResult, McpError> {
        if name != "explain_addition" {
            return Err(McpError::McpError(ErrorData::invalid_params(
                format!("unknown calculator prompt {name:?}"),
                None,
            )));
        }
        Ok(GetPromptResult::new(vec![
            PromptMessage::new_text(Role::User, "What is two plus two?"),
            PromptMessage::new_text(Role::Assistant, "Four."),
            PromptMessage::new_text(Role::User, "Why?"),
        ]))
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }

    async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
        let (tx, rx) = mpsc::channel(32);
        self.notification_senders.lock().unwrap().push(tx);
        rx
    }
}
