//! Exposes extension capabilities and executes requests that belong to them.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::{FutureExt, StreamExt};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, ErrorData, Role, Tool};

use crate::agents::extension_manager::ExtensionManager;
use crate::agents::platform_extensions::MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE;
use crate::agents::state_machine::ops_llm::{ADVERTISED_TOOLS_NOTE, LLM_OPERATION_NAME};
use crate::agents::state_machine::ops_tool_approval::request_executable;
use crate::agents::state_machine::{
    applied, messages_since_kickoff, not_applicable, yielded_with, ConversationEffect, Emitter,
    GooseEffect, Operation, OperationResult, SlashCommand,
};
use crate::agents::tool_execution::{
    tool_stream, ToolCallResult, ToolStreamItem, CHAT_MODE_TOOL_SKIPPED_RESPONSE, DECLINED_RESPONSE,
};
use crate::agents::AgentEvent;
use crate::config::GooseMode;
use crate::conversation::message::{ActionRequiredData, Message, MessageContent, ToolRequest};
use crate::conversation::Conversation;
use crate::hints::load_hints::SubdirectoryHintTracker;
use crate::hooks::{HookChainOutcome, HookContext, HookEvent, HookManager};
use crate::session::{EnabledExtensionsState, ExtensionState, Session};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing_futures::Instrument;

#[derive(Clone, Copy)]
enum ToolCategory {
    Shell,
    Read,
    Write,
    Other,
}

fn categorize_tool(tool_name: &str) -> ToolCategory {
    match tool_name.rsplit("__").next().unwrap_or(tool_name) {
        "shell" | "bash" | "exec" | "run" => ToolCategory::Shell,
        "read" | "view" | "cat" | "read_file" => ToolCategory::Read,
        "write" | "edit" | "patch" | "write_file" | "edit_file" => ToolCategory::Write,
        _ => ToolCategory::Other,
    }
}

fn string_argument(input: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let arguments = input.as_object()?;
    keys.iter().find_map(|key| {
        arguments
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn platform_notification(result: &CallToolResult) -> Option<rmcp::model::ServerNotification> {
    let notification = result.meta.as_ref()?.0.get("platform_notification")?;
    let method = notification.get("method")?.as_str()?;
    Some(rmcp::model::ServerNotification::CustomNotification(
        rmcp::model::CustomNotification::new(
            method.to_string(),
            notification.get("params").cloned(),
        ),
    ))
}

pub(super) fn tool_span(tool_name: &str, tool_call_id: &str, session_id: &str) -> tracing::Span {
    tracing::info_span!(
        target: "goose::state_machine",
        "execute_tool",
        "gen_ai.operation.name" = "execute_tool",
        "gen_ai.tool.name" = %tool_name,
        "gen_ai.tool.call.id" = %tool_call_id,
        "gen_ai.tool.call.arguments" = tracing::field::Empty,
        "gen_ai.tool.call.result" = tracing::field::Empty,
        "error.type" = tracing::field::Empty,
        session.id = %session_id,
    )
}

/// Observation-only record of what the `PreToolUse` chain decided. Carries
/// no veto: the decision has already been made by the time this runs.
async fn emit_pre_tool_use_result(
    hook_manager: &HookManager,
    session: &Session,
    tool_call_id: &str,
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
    outcome: &HookChainOutcome,
) {
    if !hook_manager.has_hooks(HookEvent::PreToolUseResult) {
        return;
    }
    let context = HookContext::new(HookEvent::PreToolUseResult, &session.id)
        .with_tool(tool_name.to_string(), tool_input.cloned())
        .with_tool_call_id(tool_call_id)
        .with_working_dir(session.working_dir.to_string_lossy().to_string())
        .with_pre_tool_use_outcome(outcome);
    hook_manager.emit_pre_tool_use_result(context).await;
}

/// Runs the `PreToolUse` chain, emits `PreToolUseResult`, and reports a denial
/// as the error the caller must return instead of executing.
///
/// Shared rather than duplicated because it carries policy: which decisions
/// block, what `policy_evaluated` means, and that the result event is emitted
/// on both the allow and the deny path. `RecipeOperation` executes
/// `recipe__final_output` without going through [`ToolExecutionOperation`], so
/// it calls this to get the identical lifecycle rather than its own copy.
pub(super) async fn run_pre_tool_hooks(
    hook_manager: &HookManager,
    session: &Session,
    tool_call_id: &str,
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
) -> std::result::Result<(), ErrorData> {
    let outcome = if hook_manager.has_hooks(HookEvent::PreToolUse) {
        let context = HookContext::new(HookEvent::PreToolUse, &session.id)
            .with_tool(tool_name.to_string(), tool_input.cloned())
            .with_tool_call_id(tool_call_id)
            .with_working_dir(session.working_dir.to_string_lossy().to_string());
        hook_manager
            .emit_blocking_with_outcome(HookEvent::PreToolUse, context)
            .await
    } else {
        HookChainOutcome::allow(false)
    };

    // Emitted before the denial returns, so an observer sees the denial
    // before the model receives the refusal. Best effort, like every other
    // hook emission: a subscriber that fails or is absent changes nothing.
    emit_pre_tool_use_result(
        hook_manager,
        session,
        tool_call_id,
        tool_name,
        tool_input,
        &outcome,
    )
    .await;

    if let Some(denial) = outcome.denial() {
        tracing::Span::current().record("error.type", denial.error_type);
        return Err(ErrorData::new(
            rmcp::model::ErrorCode::INTERNAL_ERROR,
            denial.message,
            None,
        ));
    }
    Ok(())
}

async fn emit_with_matcher(
    hook_manager: &HookManager,
    event: HookEvent,
    session: &Session,
    matcher: String,
    tool_name: &str,
    tool_input: Option<serde_json::Value>,
) {
    if !hook_manager.has_hooks(event) {
        return;
    }
    let mut context = HookContext::new(event, &session.id)
        .with_tool(tool_name.to_string(), tool_input)
        .with_working_dir(session.working_dir.to_string_lossy().to_string());
    context.matcher_context = Some(matcher);
    hook_manager.emit(event, context).await;
}

pub(super) async fn emit_extended_pre_hooks(
    hook_manager: &HookManager,
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
    session: &Session,
) {
    let (event, matcher) = match categorize_tool(tool_name) {
        ToolCategory::Shell => (
            HookEvent::BeforeShellExecution,
            tool_input.and_then(|input| string_argument(input, &["command"])),
        ),
        ToolCategory::Read => (
            HookEvent::BeforeReadFile,
            tool_input.and_then(|input| string_argument(input, &["path", "file", "file_path"])),
        ),
        ToolCategory::Write | ToolCategory::Other => return,
    };
    if let Some(matcher) = matcher {
        emit_with_matcher(
            hook_manager,
            event,
            session,
            matcher,
            tool_name,
            tool_input.cloned(),
        )
        .await;
    }
}

/// Emits the post-tool event for a finished call and reports which one fired.
///
/// Which event fires is policy: a result flagged `is_error` counts as a failure,
/// as does a transport error. Shared so every execution path classifies the
/// outcome the same way. Takes the session id and working dir as strings because
/// the caller inside [`with_post_tool_hooks`] holds owned copies, not a session.
pub(super) async fn emit_post_tool_use(
    hook_manager: &HookManager,
    session_id: &str,
    working_dir: &str,
    tool_name: &str,
    tool_call_id: &str,
    tool_input: Option<&serde_json::Value>,
    result: &std::result::Result<CallToolResult, ErrorData>,
) -> HookEvent {
    let event = match result {
        Ok(result) if result.is_error != Some(true) => HookEvent::PostToolUse,
        _ => HookEvent::PostToolUseFailure,
    };
    if hook_manager.has_hooks(event) {
        let context = HookContext::new(event, session_id)
            .with_tool(tool_name.to_string(), tool_input.cloned())
            .with_tool_call_id(tool_call_id)
            .with_working_dir(working_dir.to_string());
        hook_manager.emit(event, context).await;
    }
    event
}

/// Wraps a tool result so the post-tool event fires once the call completes,
/// carrying the same `tool_call_id` the pre events carried.
pub(super) fn with_post_tool_hooks(
    hook_manager: &HookManager,
    result: ToolCallResult,
    tool_call: &CallToolRequestParams,
    session: &Session,
    span: tracing::Span,
    tool_call_id: &str,
) -> ToolCallResult {
    let hook_manager = hook_manager.clone();
    let session_id = session.id.clone();
    let working_dir = session.working_dir.to_string_lossy().to_string();
    let tool_name = tool_call.name.to_string();
    let tool_call_id = tool_call_id.to_string();
    let tool_input = tool_call
        .arguments
        .as_ref()
        .map(|arguments| serde_json::Value::Object(arguments.clone()));
    let category = categorize_tool(&tool_name);
    let future = async move {
        let result =
            crate::agents::large_response_handler::process_tool_response(result.result.await);
        crate::agents::gen_ai_telemetry::record_tool_result(&tracing::Span::current(), &result);
        match &result {
            Ok(result) if result.is_error == Some(true) => {
                tracing::Span::current().record("error.type", "tool_error");
            }
            Err(_) => {
                tracing::Span::current().record("error.type", "tool_execution_error");
            }
            _ => {}
        }
        let event = emit_post_tool_use(
            &hook_manager,
            &session_id,
            &working_dir,
            &tool_name,
            &tool_call_id,
            tool_input.as_ref(),
            &result,
        )
        .await;

        if event == HookEvent::PostToolUse {
            let extended = match category {
                ToolCategory::Shell => Some((
                    HookEvent::AfterShellExecution,
                    tool_input
                        .as_ref()
                        .and_then(|input| string_argument(input, &["command"])),
                )),
                ToolCategory::Write => Some((
                    HookEvent::AfterFileEdit,
                    tool_input
                        .as_ref()
                        .and_then(|input| string_argument(input, &["path", "file", "file_path"])),
                )),
                ToolCategory::Read | ToolCategory::Other => None,
            };
            if let Some((event, Some(matcher))) = extended {
                if hook_manager.has_hooks(event) {
                    let mut context = HookContext::new(event, &session_id)
                        .with_tool(tool_name, tool_input)
                        .with_working_dir(working_dir);
                    context.matcher_context = Some(matcher);
                    hook_manager.emit(event, context).await;
                }
            }
        }
        result
    }
    .instrument(span);
    ToolCallResult {
        notification_stream: result.notification_stream,
        action_required_stream: result.action_required_stream,
        result: Box::new(future.boxed()),
    }
}

pub struct ToolExecutionOperation<'a> {
    goose_mode: &'a Mutex<GooseMode>,
    extension_manager: Arc<ExtensionManager>,
    hook_manager: HookManager,
}

impl<'a> ToolExecutionOperation<'a> {
    pub fn new(
        goose_mode: &'a Mutex<GooseMode>,
        extension_manager: Arc<ExtensionManager>,
        hook_manager: HookManager,
    ) -> Self {
        Self {
            goose_mode,
            extension_manager,
            hook_manager,
        }
    }

    async fn dispatch_tool_call(
        &self,
        tool_call: CallToolRequestParams,
        request_id: String,
        cancellation_token: CancellationToken,
        session: &Session,
    ) -> std::result::Result<ToolCallResult, ErrorData> {
        let span = tool_span(&tool_call.name, &request_id, &session.id);
        crate::agents::gen_ai_telemetry::record_tool_arguments(&span, &tool_call);
        let result_span = span.clone();

        async {
            let tool_input = tool_call
                .arguments
                .as_ref()
                .map(|arguments| serde_json::Value::Object(arguments.clone()));
            run_pre_tool_hooks(
                &self.hook_manager,
                session,
                request_id.as_str(),
                &tool_call.name,
                tool_input.as_ref(),
            )
            .await?;

            emit_extended_pre_hooks(
                &self.hook_manager,
                &tool_call.name,
                tool_input.as_ref(),
                session,
            )
            .await;

            let context = crate::agents::tool_execution::ToolCallContext::new(
                session.id.clone(),
                Some(session.working_dir.clone()),
                Some(request_id.clone()),
            );
            let result = self
                .extension_manager
                .dispatch_tool_call(&context, tool_call.clone(), cancellation_token)
                .await;
            let result = result.unwrap_or_else(|error| {
                #[cfg(feature = "telemetry")]
                crate::posthog::emit_error(
                    "tool_execution_failed",
                    &format!("{}: {}", tool_call.name, error),
                );
                ToolCallResult::from(Err(error))
            });
            Ok(with_post_tool_hooks(
                &self.hook_manager,
                result,
                &tool_call,
                session,
                result_span,
                &request_id,
            ))
        }
        .instrument(span)
        .await
    }

    async fn extension_state_effect(&self, session: &Session) -> Result<GooseEffect> {
        let extension_configs = self.extension_manager.get_extension_configs().await;
        let extensions_state = EnabledExtensionsState::new(extension_configs);
        let mut extension_data = session.extension_data.clone();
        extensions_state.to_extension_data(&mut extension_data)?;
        Ok(GooseEffect::SetExtensionData(extension_data))
    }

    async fn command_response(
        conversation: &Conversation,
        message: String,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let command = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("prompt command conversation has no kickoff message"))?;
        let message_id = command
            .id
            .clone()
            .ok_or_else(|| anyhow!("Persisted slash command message has no id"))?;
        let command = command.with_visibility(true, false);
        let response = Message::assistant()
            .with_text(message)
            .with_visibility(true, false);
        emit.message(command).await;
        let response = emit.message(response).await;
        yielded_with([
            ConversationEffect::SetMessageVisibility {
                message_id,
                user_visible: true,
                agent_visible: false,
            }
            .into(),
            response.into(),
        ])
    }

    async fn list_prompts(
        &self,
        command: &SlashCommand<'_>,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let prompts = match self
            .extension_manager
            .list_prompts(&session.id, emit.cancel_token().clone())
            .await
        {
            Ok(prompts) => prompts,
            Err(error) => {
                return Self::command_response(
                    conversation,
                    format!("Failed to list prompts: {}", error.message),
                    emit,
                )
                .await;
            }
        };
        let extension_filter = command.params_str.split_whitespace().next();
        if let Some(filter) = extension_filter {
            if !prompts.contains_key(filter) {
                return Self::command_response(
                    conversation,
                    format!("Extension '{filter}' not found"),
                    emit,
                )
                .await;
            }
        }

        let filtered: HashMap<_, _> = prompts
            .into_iter()
            .filter(|(extension, _)| extension_filter.is_none_or(|filter| extension == filter))
            .collect();
        let mut output = String::new();
        if filtered.is_empty() {
            output.push_str("No prompts available.\n");
        } else {
            output.push_str("Available prompts:\n\n");
            for (extension, prompts) in filtered {
                output.push_str(&format!("**{extension}**:\n"));
                for prompt in prompts {
                    output.push_str(&format!("  - {}\n", prompt.name));
                }
                output.push('\n');
            }
        }
        Self::command_response(conversation, output, emit).await
    }

    async fn run_prompt(
        &self,
        command: &SlashCommand<'_>,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let params: Vec<_> = command.params_str.split_whitespace().collect();
        let Some(prompt_name) = params.first() else {
            return Self::command_response(
                conversation,
                "Prompt name argument is required".to_string(),
                emit,
            )
            .await;
        };
        let prompts = match self
            .extension_manager
            .list_prompts(&session.id, emit.cancel_token().clone())
            .await
        {
            Ok(prompts) => prompts,
            Err(error) => {
                return Self::command_response(
                    conversation,
                    format!("Failed to list prompts: {}", error.message),
                    emit,
                )
                .await;
            }
        };
        let found = prompts.iter().find_map(|(extension, prompts)| {
            prompts
                .iter()
                .find(|prompt| prompt.name == *prompt_name)
                .map(|prompt| (extension.clone(), prompt.clone()))
        });
        let Some((extension, prompt)) = found else {
            return Self::command_response(
                conversation,
                format!("Prompt '{prompt_name}' not found"),
                emit,
            )
            .await;
        };

        if params.get(1) == Some(&"--info") {
            let mut output = format!("**Prompt: {}**\n\n", prompt.name);
            if let Some(description) = &prompt.description {
                output.push_str(&format!("Description: {description}\n\n"));
            }
            output.push_str(&format!("Extension: {extension}\n\n"));
            if let Some(arguments) = &prompt.arguments {
                output.push_str("Arguments:\n");
                for argument in arguments {
                    output.push_str(&format!("  - {}", argument.name));
                    if let Some(description) = &argument.description {
                        output.push_str(&format!(": {description}"));
                    }
                    output.push('\n');
                }
            }
            return Self::command_response(conversation, output, emit).await;
        }

        let arguments: HashMap<_, _> = params
            .iter()
            .skip(1)
            .filter_map(|param| param.split_once('='))
            .map(|(key, value)| (key.to_string(), value.trim_matches('"').to_string()))
            .collect();
        let result = match self
            .extension_manager
            .get_prompt(
                &session.id,
                &extension,
                prompt_name,
                serde_json::to_value(arguments)?,
                emit.cancel_token().clone(),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                return Self::command_response(conversation, error.to_string(), emit).await;
            }
        };

        let command_message = messages_since_kickoff(conversation)?
            .first()
            .ok_or_else(|| anyhow!("prompt command conversation has no kickoff message"))?;
        let message_id = command_message
            .id
            .clone()
            .ok_or_else(|| anyhow!("Persisted slash command message has no id"))?;
        let mut effects = vec![ConversationEffect::SetMessageVisibility {
            message_id,
            user_visible: true,
            agent_visible: false,
        }
        .into()];
        for (index, prompt_message) in result.messages.into_iter().enumerate() {
            let message = Message::from(prompt_message);
            let expected_role = if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            if message.role != expected_role {
                return Self::command_response(
                    conversation,
                    format!(
                        "Expected {expected_role:?} message at position {index}, but found {:?}",
                        message.role
                    ),
                    emit,
                )
                .await;
            }
            effects.push(message.with_visibility(false, true).into());
        }
        if effects.len() == 1 {
            return Self::command_response(
                conversation,
                format!("Prompt '{prompt_name}' returned no messages"),
                emit,
            )
            .await;
        }
        applied(effects)
    }
}

pub(super) fn pending_tool_requests(messages: &[Message]) -> Vec<(ToolRequest, ToolDisposition)> {
    let mut answered = HashSet::new();
    let mut approval_requests = HashSet::new();
    let mut approvals = std::collections::HashMap::new();
    for message in messages {
        for content in &message.content {
            match content {
                MessageContent::ToolResponse(response) => {
                    answered.insert(response.id.clone());
                }
                MessageContent::ActionRequired(action) => match &action.data {
                    ActionRequiredData::ToolConfirmation { id, .. } => {
                        approval_requests.insert(id.clone());
                    }
                    ActionRequiredData::ToolConfirmationResponse { id, permission } => {
                        approvals.insert(id.clone(), permission.clone());
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .flat_map(|message| {
            message.content.iter().filter_map(|c| match c {
                MessageContent::ToolRequest(req) if req.was_executed_externally() => None,
                MessageContent::ToolRequest(req) if !answered.contains(&req.id) => {
                    if let Err(parse_error) = &req.tool_call {
                        return Some((
                            req.clone(),
                            ToolDisposition::ParseError(parse_error.to_string()),
                        ));
                    }
                    match request_executable(req).unwrap_or(true) {
                        true => Some((req.clone(), ToolDisposition::Execute)),
                        false => {
                            if approval_requests.contains(&req.id)
                                && !approval_denied(approvals.get(&req.id))
                            {
                                None
                            } else {
                                Some((req.clone(), ToolDisposition::Decline))
                            }
                        }
                    }
                }
                _ => None,
            })
        })
        .collect()
}

pub(super) fn pending_advertised_tool_requests(
    messages: &[Message],
) -> Vec<(ToolRequest, ToolDisposition)> {
    pending_tool_requests(messages)
        .into_iter()
        .filter(|(request, _)| request_was_advertised(messages, request))
        .collect()
}

pub(super) fn request_was_advertised(messages: &[Message], request: &ToolRequest) -> bool {
    let Some(tool_call) = request.tool_call.as_ref().ok() else {
        return true;
    };
    let Some(message) = messages.iter().find(|message| {
        message.role == Role::Assistant
            && message.content.iter().any(|content| {
                content
                    .as_tool_request()
                    .is_some_and(|item| item.id == request.id)
            })
    }) else {
        return false;
    };
    if message.metadata.inference.is_none() {
        return true;
    }
    message
        .metadata
        .operation_note(LLM_OPERATION_NAME, ADVERTISED_TOOLS_NOTE)
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.as_str() == Some(tool_call.name.as_ref()))
        })
}

#[derive(Clone, Eq, PartialEq)]
pub(super) enum ToolDisposition {
    Execute,
    Decline,
    ParseError(String),
}

fn approval_denied(permission: Option<&crate::permission::Permission>) -> bool {
    matches!(
        permission,
        Some(
            crate::permission::Permission::DenyOnce
                | crate::permission::Permission::AlwaysDeny
                | crate::permission::Permission::Cancel
        )
    )
}

#[async_trait]
impl Operation<Session, GooseEffect> for ToolExecutionOperation<'_> {
    fn name(&self) -> &'static str {
        "tool_execution"
    }

    async fn run_command(
        &self,
        command: &SlashCommand<'_>,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        match command.command {
            "prompts" => {
                self.list_prompts(command, session, conversation, emit)
                    .await
            }
            "prompt" => self.run_prompt(command, session, conversation, emit).await,
            _ => not_applicable(),
        }
    }

    async fn inference_tools(&self, session: &Session) -> Result<Vec<Tool>> {
        let tools = self
            .extension_manager
            .get_prefixed_tools_excluding(&session.id, crate::skills::EXTENSION_NAME)
            .await
            .unwrap_or_default();
        Ok(tools)
    }

    async fn moim_parts(
        &self,
        session: &Session,
        _conversation: &Conversation,
    ) -> Result<Vec<String>> {
        Ok(self.extension_manager.collect_moim_parts(&session.id).await)
    }

    async fn prompt_parts(
        &self,
        session: &Session,
        conversation: &Conversation,
    ) -> Result<Vec<(String, String)>> {
        let mut hints = SubdirectoryHintTracker::new();
        for message in conversation.messages() {
            for content in &message.content {
                if let MessageContent::ToolRequest(request) = content {
                    if let Ok(tool_call) = &request.tool_call {
                        hints.record_tool_arguments(&tool_call.arguments, &session.working_dir);
                    }
                }
            }
        }
        let mut prompt_parts = hints.load_new_hints(&session.working_dir);

        #[cfg(feature = "code-mode")]
        if self
            .extension_manager
            .is_extension_enabled(
                crate::agents::platform_extensions::code_execution::EXTENSION_NAME,
            )
            .await
        {
            return Ok(prompt_parts);
        }

        let mut extensions = self
            .extension_manager
            .get_extensions_info(&session.working_dir)
            .await;
        extensions.retain(|extension| extension.name != crate::skills::EXTENSION_NAME);
        if extensions.is_empty() {
            return Ok(prompt_parts);
        }
        // HashMap order shuffles across restarts and would bust the prompt cache.
        extensions.sort_by(|a, b| a.name.cmp(&b.name));

        let mut lines = vec![
            "# Extensions".to_string(),
            "Extensions provide additional tools and context from different data sources and applications.\n\
             You can dynamically enable or disable extensions as needed to help complete tasks.\n\n\
             Because you dynamically load extensions, your conversation history may refer to interactions with extensions that are not currently active. The currently active extensions are below. Each of these extensions provides tools that are in your tool specification."
                .to_string(),
        ];
        for extension in extensions {
            lines.push(format!("## {}", extension.name));
            if extension.has_resources {
                lines.push(format!("{} supports resources.", extension.name));
            }
            if !extension.instructions.is_empty() {
                lines.push(format!("### Instructions\n{}", extension.instructions));
            }
        }
        prompt_parts.push(("extensions".to_string(), lines.join("\n\n")));
        Ok(prompt_parts)
    }

    async fn run(
        &self,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let messages = messages_since_kickoff(conversation)?;
        let mut pending = pending_advertised_tool_requests(messages);
        if pending.is_empty() {
            return not_applicable();
        }

        let known_tools: HashSet<_> = self
            .extension_manager
            .get_prefixed_tools_excluding(&session.id, crate::skills::EXTENSION_NAME)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        pending.retain(|(request, _)| {
            request
                .tool_call
                .as_ref()
                .is_ok_and(|tool_call| known_tools.contains(tool_call.name.as_ref()))
        });
        let requests: Vec<_> = pending.iter().map(|(request, _)| request.clone()).collect();
        if requests.is_empty() {
            return not_applicable();
        }

        if *self.goose_mode.lock().await == GooseMode::Chat {
            let mut response = Message::user();
            for (request, disposition) in &pending {
                let result = match disposition {
                    ToolDisposition::ParseError(parse_error) => {
                        CallToolResult::error(vec![ContentBlock::text(format!(
                            "The tool call could not be parsed: {parse_error}."
                        ))])
                    }
                    _ => CallToolResult::success(vec![ContentBlock::text(
                        CHAT_MODE_TOOL_SKIPPED_RESPONSE,
                    )]),
                };
                response.add_tool_response_with_metadata(
                    request.id.clone(),
                    Ok(result),
                    request.metadata.as_ref(),
                );
            }
            let response = emit.message(response).await;
            return applied([response.into()]);
        }

        let manage_extensions_ids: HashSet<&str> = pending
            .iter()
            .filter_map(|(request, _)| match &request.tool_call {
                Ok(tool_call) if tool_call.name == MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE => {
                    Some(request.id.as_str())
                }
                _ => None,
            })
            .collect();
        let mut extension_change_failed = false;

        let mut tool_streams = Vec::new();
        for (request, disposition) in &pending {
            if *disposition != ToolDisposition::Execute {
                continue;
            }
            let tool_call = request
                .tool_call
                .clone()
                .map_err(|e| anyhow!("tool call could not be parsed: {e}"))?;
            let result = self
                .dispatch_tool_call(
                    tool_call,
                    request.id.clone(),
                    emit.cancel_token().clone(),
                    session,
                )
                .await;
            let result = result.unwrap_or_else(|error_data| ToolCallResult::from(Err(error_data)));

            let req_id = request.id.clone();
            let stream = tool_stream(
                result
                    .notification_stream
                    .unwrap_or_else(|| Box::new(futures::stream::empty())),
                result
                    .action_required_stream
                    .unwrap_or_else(|| Box::new(futures::stream::empty())),
                result.result,
            )
            .map(move |item| (req_id.clone(), item));
            tool_streams.push(stream);
        }

        let mut combined = futures::stream::select_all(tool_streams);
        let mut response = Message::user();
        let mut effects = Vec::new();
        for (request, disposition) in &pending {
            match disposition {
                ToolDisposition::Execute => {}
                ToolDisposition::Decline => {
                    response.add_tool_response_with_metadata(
                        request.id.clone(),
                        Ok(CallToolResult::error(vec![ContentBlock::text(
                            DECLINED_RESPONSE,
                        )])),
                        request.metadata.as_ref(),
                    );
                }
                ToolDisposition::ParseError(parse_error) => {
                    response.add_tool_response_with_metadata(
                        request.id.clone(),
                        Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                            "The tool call could not be parsed: {parse_error}. \
                             Correct the arguments and try again."
                        ))])),
                        request.metadata.as_ref(),
                    );
                }
            }
        }

        loop {
            tokio::select! {
                biased;
                item = combined.next() => {
                    let Some((request_id, item)) = item else { break };
                    match item {
                        ToolStreamItem::Result(output) => {
                            if manage_extensions_ids.contains(request_id.as_str())
                                && output.is_err()
                            {
                                extension_change_failed = true;
                            }
                            if let Ok(result) = &output {
                                if let Some(notification) = platform_notification(result) {
                                    emit.emit(AgentEvent::McpNotification((
                                        request_id.clone(),
                                        notification,
                                    )))
                                    .await;
                                }
                            }
                            let metadata = requests
                                .iter()
                                .find(|r| r.id == request_id)
                                .and_then(|r| r.metadata.as_ref());
                            response.add_tool_response_with_metadata(request_id, output, metadata);
                        }
                        ToolStreamItem::Message(msg) => {
                            emit.emit(AgentEvent::McpNotification((request_id, msg)))
                                .await;
                        }
                        ToolStreamItem::ActionRequired(msg) => {
                            let msg = emit.message(msg).await;
                            effects.push(msg.into());
                        }
                    }
                },
                _ = emit.cancelled() => break,
            }
        }

        let answered: HashSet<String> = response
            .get_tool_response_ids()
            .into_iter()
            .map(str::to_string)
            .collect();
        for request in &requests {
            if !answered.contains(request.id.as_str()) {
                response.add_tool_response_with_metadata(
                    request.id.clone(),
                    Ok(CallToolResult::error(vec![ContentBlock::text(
                        "Tool call was interrupted before completing",
                    )])),
                    request.metadata.as_ref(),
                );
            }
        }

        if !manage_extensions_ids.is_empty() && !extension_change_failed {
            effects.push(self.extension_state_effect(session).await?);
        }

        let response = response.with_generated_id_if_missing();
        emit.emit(AgentEvent::Message(response.user_visible_content()))
            .await;
        effects.push(response.into());
        applied(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externally_dispatched_observations_are_not_pending_execution() {
        use crate::conversation::message::TOOL_META_EXTERNAL_DISPATCH_KEY;

        let message = Message::assistant().with_tool_request_with_metadata(
            "external",
            Ok(CallToolRequestParams::new("registered__tool")),
            None,
            Some(serde_json::json!({ TOOL_META_EXTERNAL_DISPATCH_KEY: true })),
        );

        assert!(pending_tool_requests(&[message]).is_empty());
    }

    #[test]
    fn operation_generated_requests_without_inference_remain_executable() {
        let message = Message::assistant().with_tool_request(
            "operation-generated",
            Ok(CallToolRequestParams::new("registered__tool")),
        );

        let pending = pending_advertised_tool_requests(&[message]);

        assert_eq!(pending.len(), 1);
        assert!(matches!(pending[0].1, ToolDisposition::Execute));
    }

    #[test]
    fn reads_platform_notification_from_tool_result() {
        let meta = serde_json::json!({
            "platform_notification": {
                "method": "platform_event",
                "params": { "event_type": "app_updated" }
            }
        });
        let result = CallToolResult::success(Vec::new()).with_meta(Some(rmcp::model::MetaObject(
            meta.as_object().unwrap().clone(),
        )));

        let Some(rmcp::model::ServerNotification::CustomNotification(notification)) =
            platform_notification(&result)
        else {
            panic!("expected a custom notification");
        };
        assert_eq!(notification.method, "platform_event");
        assert_eq!(
            notification.params,
            Some(serde_json::json!({ "event_type": "app_updated" }))
        );
    }
}
