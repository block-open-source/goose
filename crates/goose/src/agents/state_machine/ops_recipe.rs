//! Applies recipe commands and enforces their structured final output.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use tracing_futures::Instrument;

use crate::agents::final_output_tool::{
    structured_output_unsupported_message, FinalOutputTool, FINAL_OUTPUT_CONTINUATION_MESSAGE,
    FINAL_OUTPUT_SUCCESS_MESSAGE, FINAL_OUTPUT_TOOL_NAME,
};
use crate::agents::state_machine::ops_toolcalling::{
    emit_post_tool_use, pending_advertised_tool_requests, run_pre_tool_hooks, tool_span,
    ToolDisposition,
};
use crate::agents::state_machine::{
    applied, ends_turn, last_effective_role, messages_since_kickoff, not_applicable, yielded_with,
    ConversationEffect, Emitter, GooseEffect, Operation, OperationResult, SlashCommand,
};
use crate::agents::tool_execution::CHAT_MODE_TOOL_SKIPPED_RESPONSE;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageContent};
use crate::conversation::{Conversation, EffectiveRole};
use crate::hooks::HookManager;
use crate::providers::base::Provider;
use crate::session::Session;

pub struct RecipeOperation {
    provider: Arc<dyn Provider>,
    hook_manager: HookManager,
}

impl RecipeOperation {
    pub fn new(provider: Arc<dyn Provider>, hook_manager: HookManager) -> Self {
        Self {
            provider,
            hook_manager,
        }
    }

    fn final_output(session: &Session) -> Result<Option<FinalOutputTool>> {
        session
            .recipe
            .as_ref()
            .and_then(|recipe| recipe.response.clone())
            .map(FinalOutputTool::try_new)
            .transpose()
            .map_err(|error| anyhow!(error))
    }

    fn assistant_block_bounds(messages: &[Message], message_index: usize) -> (usize, usize) {
        let start = (0..message_index)
            .rev()
            .take_while(|index| messages[*index].role == rmcp::model::Role::Assistant)
            .last()
            .unwrap_or(message_index);
        let end = (message_index + 1..messages.len())
            .take_while(|index| messages[*index].role == rmcp::model::Role::Assistant)
            .last()
            .map_or(message_index + 1, |index| index + 1);
        (start, end)
    }

    fn has_unanswered_siblings(messages: &[Message], request_id: &str) -> bool {
        let answered: HashSet<&str> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResponse(response) => Some(response.id.as_str()),
                _ => None,
            })
            .collect();
        let Some(message_index) = messages.iter().position(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    MessageContent::ToolRequest(request) if request.id == request_id
                )
            })
        }) else {
            return false;
        };
        let (start, end) = Self::assistant_block_bounds(messages, message_index);
        messages[start..end]
            .iter()
            .flat_map(|message| &message.content)
            .any(|content| match content {
                // Another unanswered final-output call is not a reason to wait.
                // This operation drains them one per pass, so treating a sibling
                // final-output call as unfinished work would deadlock the pair:
                // each would wait for the other and neither would be answered.
                // Ordinary tool calls still have to finish first.
                MessageContent::ToolRequest(request) => {
                    request.id != request_id
                        && !answered.contains(request.id.as_str())
                        && !request
                            .tool_call
                            .as_ref()
                            .is_ok_and(|tool_call| tool_call.name == FINAL_OUTPUT_TOOL_NAME)
                }
                _ => false,
            })
    }

    pub(super) fn successful_final_output(messages: &[Message]) -> Option<String> {
        Self::successful_final_output_batch(messages).map(|(output, _)| output)
    }

    /// The newest successful final-output response and the end of the
    /// assistant block that requested it, when every request of the block
    /// has been answered.
    fn successful_final_output_batch(messages: &[Message]) -> Option<(String, usize)> {
        let answered_responses: HashSet<&str> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResponse(response) => Some(response.id.as_str()),
                _ => None,
            })
            .collect();
        let successful_responses: HashSet<&str> = messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|content| match content {
                MessageContent::ToolResponse(response)
                    if response.tool_result.as_ref().is_ok_and(|result| {
                        result.is_error != Some(true)
                            && result.content.iter().any(|content| {
                                content
                                    .as_text()
                                    .is_some_and(|text| text.text == FINAL_OUTPUT_SUCCESS_MESSAGE)
                            })
                    }) =>
                {
                    Some(response.id.as_str())
                }
                _ => None,
            })
            .collect();

        for (message_index, message) in messages.iter().enumerate().rev() {
            let output = message
                .content
                .iter()
                .rev()
                .find_map(|content| match content {
                    MessageContent::ToolRequest(request)
                        if successful_responses.contains(request.id.as_str()) =>
                    {
                        request.tool_call.as_ref().ok().and_then(|tool_call| {
                            (tool_call.name == FINAL_OUTPUT_TOOL_NAME).then(|| {
                                serde_json::Value::Object(
                                    tool_call.arguments.clone().unwrap_or_default(),
                                )
                                .to_string()
                            })
                        })
                    }
                    _ => None,
                });
            if output.is_some() {
                let (block_start, block_end) =
                    Self::assistant_block_bounds(messages, message_index);
                let siblings_answered = messages[block_start..block_end]
                    .iter()
                    .flat_map(|message| &message.content)
                    .all(|content| match content {
                        MessageContent::ToolRequest(request) => {
                            answered_responses.contains(request.id.as_str())
                        }
                        _ => true,
                    });
                return if siblings_answered {
                    output.map(|output| (output, block_end))
                } else {
                    None
                };
            }
        }
        None
    }

    /// A completed recipe result that is still waiting for its first
    /// delivery: no assistant message follows the response's block, so the
    /// recipe operation delivers it later in this pass — a drained steer may
    /// sit between the response and the delivery. An assistant message after
    /// the block means the run has moved past the response: the delivery
    /// landed, or a steer drove a new inference. Such a run — for example
    /// one a stop-hook denial resumed after a delivered result — must keep
    /// its proactive compaction checks.
    pub(super) fn final_output_awaiting_delivery(messages: &[Message]) -> bool {
        let Some((_, block_end)) = Self::successful_final_output_batch(messages) else {
            return false;
        };
        messages[block_end..]
            .iter()
            .all(|message| message.role != rmcp::model::Role::Assistant)
    }

    async fn command_error(
        &self,
        conversation: &Conversation,
        message: String,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let command = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("recipe command conversation has no kickoff message"))?;
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
}

#[async_trait]
impl Operation<Session, GooseEffect> for RecipeOperation {
    fn name(&self) -> &'static str {
        "recipe"
    }

    async fn run_command(
        &self,
        command: &SlashCommand<'_>,
        _session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let (recipe, prompt) = match crate::slash_commands::recipe_slash_command::resolve_command(
            command.command,
            command.params_str,
        ) {
            Ok(Some(recipe)) => recipe,
            Ok(None) => return not_applicable(),
            Err(error) => return self.command_error(conversation, error, emit).await,
        };

        if let Some(response) = recipe.response.clone() {
            if let Err(error) = FinalOutputTool::try_new(response) {
                return self
                    .command_error(conversation, format!("Recipe is not valid: {error}"), emit)
                    .await;
            }
        }

        #[cfg(feature = "telemetry")]
        crate::posthog::emit_custom_slash_command_used();

        let command_message = messages_since_kickoff(conversation)?
            .first()
            .ok_or_else(|| anyhow!("recipe command conversation has no kickoff message"))?;
        let message_id = command_message
            .id
            .clone()
            .ok_or_else(|| anyhow!("Persisted slash command message has no id"))?;
        applied([
            ConversationEffect::SetMessageVisibility {
                message_id,
                user_visible: true,
                agent_visible: false,
            }
            .into(),
            GooseEffect::SetRecipe(Box::new(Some(recipe))),
            Message::user()
                .with_text(prompt)
                .with_visibility(false, true)
                .into(),
        ])
    }

    async fn inference_tools(&self, session: &Session) -> Result<Vec<Tool>> {
        Ok(Self::final_output(session)?
            .as_ref()
            .map(FinalOutputTool::tool)
            .into_iter()
            .collect())
    }

    async fn prompt_parts(
        &self,
        session: &Session,
        _conversation: &Conversation,
    ) -> Result<Vec<(String, String)>> {
        Ok(Self::final_output(session)?
            .as_ref()
            .map(|tool| ("final_output".to_string(), tool.system_prompt()))
            .into_iter()
            .collect())
    }

    async fn run(
        &self,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let Some(mut final_output) = Self::final_output(session)? else {
            return not_applicable();
        };

        if !self.provider.supports_builtin_tools() {
            return self
                .command_error(
                    conversation,
                    structured_output_unsupported_message(self.provider.get_name()),
                    emit,
                )
                .await;
        }

        let messages = messages_since_kickoff(conversation)?;
        let pending = pending_advertised_tool_requests(messages).into_iter().find(
            |(request, disposition)| {
                *disposition == ToolDisposition::Execute
                    && request
                        .tool_call
                        .as_ref()
                        .is_ok_and(|tool_call| tool_call.name == FINAL_OUTPUT_TOOL_NAME)
            },
        );
        if let Some((request, _)) = pending {
            if session.goose_mode == GooseMode::Chat {
                let mut response = Message::user();
                response.add_tool_response_with_metadata(
                    request.id,
                    Ok(CallToolResult::success(vec![ContentBlock::text(
                        CHAT_MODE_TOOL_SKIPPED_RESPONSE,
                    )])),
                    request.metadata.as_ref(),
                );
                let response = emit.message(response).await;
                return applied([response.into()]);
            }
            if Self::has_unanswered_siblings(messages, &request.id) {
                return not_applicable();
            }

            let tool_call = request
                .tool_call
                .map_err(|error| anyhow!("final output tool call could not be parsed: {error}"))?;
            let span = tool_span(&tool_call.name, &request.id, &session.id);
            // `recipe__final_output` is executed here rather than by
            // ToolExecutionOperation, which is registered after this one. Run the
            // same hook lifecycle it would have run, so the state machine and the
            // legacy loop agree on what a final-output call emits.
            let tool_input = tool_call
                .arguments
                .as_ref()
                .map(|arguments| serde_json::Value::Object(arguments.clone()));
            let output = match run_pre_tool_hooks(
                &self.hook_manager,
                session,
                &request.id,
                &tool_call.name,
                tool_input.as_ref(),
            )
            .instrument(span.clone())
            .await
            {
                // A denial returns before execution and emits no post event, the
                // same shape ToolExecutionOperation has: its dispatch returns the
                // denial before the post-hook wrapper is ever applied.
                Err(denial) => Err(denial),
                Ok(()) => {
                    let result = final_output
                        .execute_tool_call(tool_call.clone())
                        .instrument(span.clone())
                        .await;
                    let output = result.result.instrument(span.clone()).await;
                    match &output {
                        Ok(result) if result.is_error == Some(true) => {
                            span.record("error.type", "tool_error");
                        }
                        Err(_) => {
                            span.record("error.type", "tool_execution_error");
                        }
                        _ => {}
                    }
                    // Post event carries the same tool_call_id as the pre events.
                    // The large-response rewrite ToolExecutionOperation applies is
                    // deliberately not reused: the recipe's structured output is
                    // the deliverable, not a payload to offload to a temp file.
                    emit_post_tool_use(
                        &self.hook_manager,
                        &session.id,
                        &session.working_dir.to_string_lossy(),
                        &tool_call.name,
                        &request.id,
                        tool_input.as_ref(),
                        &output,
                    )
                    .instrument(span.clone())
                    .await;
                    output
                }
            };
            let mut response = Message::user();
            response.add_tool_response_with_metadata(request.id, output, request.metadata.as_ref());
            let response = emit.message(response).await;
            return applied([response.into()]);
        }

        if let Some(output) = Self::successful_final_output(messages) {
            if last_effective_role(messages)? == EffectiveRole::Tool {
                let mut message = Message::assistant().with_text(output);
                crate::context_mgmt::mark_final_output_delivery(&mut message);
                let message = emit.message(message).await;
                return applied([message.into()]);
            }
            return not_applicable();
        }

        if ends_turn(messages) {
            let message = Message::user().with_text(FINAL_OUTPUT_CONTINUATION_MESSAGE);
            let message = emit.message(message).await;
            return applied([message.into()]);
        }

        not_applicable()
    }
}
