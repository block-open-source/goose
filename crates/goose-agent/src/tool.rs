use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use goose_provider_types::conversation::{
    message::{Message, MessageContent, ToolRequest},
    Conversation,
};
use rmcp::{
    handler::server::router::tool::{AsyncTool, SyncTool, ToolBase},
    model::{CallToolRequestParams, CallToolResult, ErrorData, JsonObject, Tool},
};
use serde_json::{json, Value};

use crate::operation::{
    applied, messages_since_kickoff, not_applicable, Emitter, Operation, OperationFuture,
    OperationResult,
};

fn empty_input_schema() -> Arc<JsonObject> {
    Arc::new(
        serde_json::from_value(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }))
        .expect("empty tool input schema is an object"),
    )
}

fn definition<T: ToolBase>() -> Tool {
    let mut tool = Tool::new_with_raw(
        T::name(),
        T::description(),
        T::input_schema().unwrap_or_else(empty_input_schema),
    );
    if let Some(title) = T::title() {
        tool = tool.with_title(title);
    }
    if let Some(output_schema) = T::output_schema() {
        tool = tool.with_raw_output_schema(output_schema);
    }
    if let Some(annotations) = T::annotations() {
        tool = tool.with_annotations(annotations);
    }
    if let Some(icons) = T::icons() {
        tool = tool.with_icons(icons);
    }
    if let Some(meta) = T::meta() {
        tool = tool.with_meta(meta);
    }
    tool
}

fn pending_requests(requests: Vec<ToolRequest>, tool_names: &HashSet<&str>) -> Vec<ToolRequest> {
    requests
        .into_iter()
        .filter(|request| {
            request
                .tool_call
                .as_ref()
                .map_or(true, |call| tool_names.contains(call.name.as_ref()))
        })
        .collect()
}

fn interrupted_result() -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![
        rmcp::model::ContentBlock::text("Tool call was interrupted before completing"),
    ]))
}

fn parameters<T: ToolBase>(arguments: Option<JsonObject>) -> Result<T::Parameter, ErrorData> {
    if T::input_schema().is_none() {
        return Ok(T::Parameter::default());
    }

    serde_json::from_value(Value::Object(arguments.unwrap_or_default())).map_err(|error| {
        ErrorData::invalid_params(format!("failed to deserialize parameters: {error}"), None)
    })
}

fn result<T: ToolBase>(output: Result<T::Output, T::Error>) -> Result<CallToolResult, ErrorData> {
    let output = output.map_err(Into::into)?;
    let value = serde_json::to_value(output).map_err(|error| {
        ErrorData::internal_error(format!("failed to serialize tool output: {error}"), None)
    })?;
    Ok(CallToolResult::structured(value))
}

/// Supplies tools whose definitions and implementations may vary by session.
///
/// A provider's tool names and their handlers must remain stable from an inference
/// advertisement until every tool call produced by that inference has been handled.
#[async_trait]
pub trait ToolProvider<S>: Send + Sync {
    async fn tools(&self, session: &S) -> Result<Vec<Tool>>;

    async fn call(
        &self,
        session: &S,
        request_id: &str,
        call: CallToolRequestParams,
        emit: &Emitter,
    ) -> Result<CallToolResult, ErrorData>;
}

type ToolHandler<S> = dyn for<'a> Fn(&'a S, Option<JsonObject>) -> OperationFuture<'a, Result<CallToolResult, ErrorData>>
    + Send
    + Sync;

struct RegisteredTool<S> {
    definition: Tool,
    handler: Arc<ToolHandler<S>>,
}

struct RegisteredToolProvider<S> {
    tools: Vec<RegisteredTool<S>>,
}

#[async_trait]
impl<S> ToolProvider<S> for RegisteredToolProvider<S>
where
    S: Send + Sync + 'static,
{
    async fn tools(&self, _session: &S) -> Result<Vec<Tool>> {
        Ok(self
            .tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect())
    }

    async fn call(
        &self,
        session: &S,
        _request_id: &str,
        call: CallToolRequestParams,
        _emit: &Emitter,
    ) -> Result<CallToolResult, ErrorData> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.definition.name == call.name)
            .ok_or_else(|| {
                ErrorData::invalid_params(format!("unknown tool {}", call.name), None)
            })?;
        (tool.handler)(session, call.arguments).await
    }
}

/// An agent operation that advertises and dispatches tools.
///
/// Tools can be registered from rmcp's typed tool traits, or supplied at runtime
/// by a [`ToolProvider`] whose definitions may vary by session.
pub struct ToolOperation<S> {
    registered: RegisteredToolProvider<S>,
    providers: Vec<Arc<dyn ToolProvider<S>>>,
}

impl<S> ToolOperation<S>
where
    S: Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            registered: RegisteredToolProvider { tools: Vec::new() },
            providers: Vec::new(),
        }
    }

    fn register(&mut self, tool: RegisteredTool<S>) {
        if let Some(existing) = self
            .registered
            .tools
            .iter_mut()
            .find(|existing| existing.definition.name == tool.definition.name)
        {
            *existing = tool;
        } else {
            self.registered.tools.push(tool);
        }
    }

    pub fn with_provider(mut self, provider: Arc<dyn ToolProvider<S>>) -> Self {
        self.providers.push(provider);
        self
    }

    pub fn with_sync_tool<T>(mut self) -> Self
    where
        S: Clone,
        T: SyncTool<S> + Send + Sync + 'static,
    {
        self.register(RegisteredTool {
            definition: definition::<T>(),
            handler: Arc::new(|session, arguments| {
                let session = session.clone();
                Box::pin(async move {
                    let parameters = parameters::<T>(arguments)?;
                    tokio::task::spawn_blocking(move || {
                        result::<T>(T::invoke(&session, parameters))
                    })
                    .await
                    .map_err(|error| {
                        ErrorData::internal_error(
                            format!("synchronous tool task failed: {error}"),
                            None,
                        )
                    })?
                })
            }),
        });
        self
    }

    pub fn with_async_tool<T>(mut self) -> Self
    where
        T: AsyncTool<S> + Send + Sync + 'static,
    {
        self.register(RegisteredTool {
            definition: definition::<T>(),
            handler: Arc::new(|session, arguments| {
                Box::pin(async move {
                    let parameters = parameters::<T>(arguments)?;
                    result::<T>(T::invoke(session, parameters).await)
                })
            }),
        });
        self
    }

    async fn available_tools(&self, session: &S) -> Result<Vec<(Tool, &dyn ToolProvider<S>)>> {
        let mut available = self
            .registered
            .tools(session)
            .await?
            .into_iter()
            .map(|tool| (tool, &self.registered as &dyn ToolProvider<S>))
            .collect::<Vec<_>>();
        for provider in &self.providers {
            for tool in provider.tools(session).await? {
                if available
                    .iter()
                    .any(|(available, _)| available.name == tool.name)
                {
                    anyhow::bail!("multiple tool providers registered '{}'", tool.name);
                }
                available.push((tool, provider.as_ref()));
            }
        }
        Ok(available)
    }
}

impl<S> Default for ToolOperation<S>
where
    S: Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S, E> Operation<S, E> for ToolOperation<S>
where
    S: Send + Sync + 'static,
    E: From<Message> + Send + 'static,
{
    fn name(&self) -> &'static str {
        "tools"
    }

    async fn inference_tools(&self, session: &S) -> Result<Vec<Tool>> {
        Ok(self
            .available_tools(session)
            .await?
            .into_iter()
            .map(|(tool, _)| tool)
            .collect())
    }

    async fn run(
        &self,
        session: &S,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        let turn = messages_since_kickoff(conversation)?;
        let answered: HashSet<&str> = turn
            .iter()
            .flat_map(Message::get_tool_response_ids)
            .collect();
        let pending = turn
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(MessageContent::as_tool_request)
            .filter(|request| {
                !request.was_executed_externally() && !answered.contains(request.id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return not_applicable();
        }

        let available = tokio::select! {
            biased;
            _ = emit.cancelled() => {
                let mut message = Message::user();
                for request in pending {
                    message.add_tool_response_with_metadata(
                        request.id,
                        interrupted_result(),
                        request.metadata.as_ref(),
                    );
                }
                let message = emit.message(message).await;
                return applied([E::from(message)]);
            },
            available = self.available_tools(session) => available?,
        };
        let available_names = available
            .iter()
            .map(|(tool, _)| tool.name.as_ref())
            .collect();
        let pending = pending_requests(pending, &available_names);
        if pending.is_empty() {
            return not_applicable();
        }

        let mut message = Message::user();
        let mut cancelled = false;
        for request in pending {
            let provider = match request.tool_call.as_ref() {
                Ok(call) => available
                    .iter()
                    .find(|(tool, _)| tool.name == call.name)
                    .map(|(_, provider)| *provider)
                    .expect("pending requests were filtered by available tools"),
                Err(_) => &self.registered as &dyn ToolProvider<S>,
            };
            let tool_result = if cancelled || emit.cancel_token().is_cancelled() {
                cancelled = true;
                interrupted_result()
            } else {
                match request.tool_call.as_ref() {
                    Err(error) => Err(error.clone()),
                    Ok(call) => {
                        tokio::select! {
                            biased;
                            _ = emit.cancelled() => {
                                cancelled = true;
                                interrupted_result()
                            },
                            result = provider.call(session, &request.id, call.clone(), emit) => result,
                        }
                    }
                }
            };
            message.add_tool_response_with_metadata(
                request.id,
                tool_result,
                request.metadata.as_ref(),
            );
        }
        let message = emit.message(message).await;
        applied([E::from(message)])
    }
}
