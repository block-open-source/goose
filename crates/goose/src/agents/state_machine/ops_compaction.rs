//! Compacts conversation history when it is too large for the configured context window.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tracing::{debug, error};
use tracing_futures::Instrument;

use crate::agents::state_machine::ops_llm::{chat_span, record_chat_usage};
use crate::agents::state_machine::ops_toolcalling::pending_tool_requests;
use crate::agents::state_machine::{
    applied, last_effective_role, messages_since_kickoff, not_applicable, trailing_error, yielded,
    yielded_with, ConversationEffect, Emitter, GooseEffect, Operation, OperationResult,
    RecipeOperation, SlashCommand,
};
use crate::context_mgmt::compact_messages;
use crate::conversation::message::{Message, MessageErrorKind, SystemNotificationType};
use crate::conversation::{Conversation, EffectiveRole};
use crate::providers::base::Provider;
use crate::session::Session;
use goose_providers::model::ModelConfig;

const COMPACTION_THINKING_TEXT: &str = "goose is compacting the conversation...";

pub(super) const MAX_CONTEXT_ERROR_COMPACTIONS: usize = 2;

/// Messages since the kickoff that started the current run. Steers land
/// inside a run and, being user messages, redefine `messages_since_kickoff`,
/// which would hide the run's earlier messages from the checks below: a
/// completed recipe result or an unanswered sibling request from before the
/// steer must still be visible to them.
fn messages_since_run_start(conversation: &Conversation) -> Result<&[Message]> {
    let messages = conversation.messages();
    let start = messages
        .iter()
        .rposition(|message| {
            message.role == rmcp::model::Role::User
                && message.is_user_visible()
                && !message.is_tool_response()
                && !message.metadata.steer
        })
        .ok_or_else(|| anyhow!("state machine conversation has no kickoff message"))?;
    Ok(&messages[start..])
}

fn compaction_part(
    total_tokens: Option<i32>,
    context_limit: usize,
    threshold: f64,
) -> Option<String> {
    let total_tokens = total_tokens?;
    if total_tokens <= 0 || context_limit == 0 || threshold <= 0.0 || threshold >= 1.0 {
        return None;
    }

    let compaction_at = (context_limit as f64 * threshold) as i32;
    if compaction_at <= 0 || (total_tokens as f64 / compaction_at as f64) < 0.5 {
        return None;
    }

    Some(format!(
        "<compaction>~{}k tokens remaining</compaction>",
        compaction_at.saturating_sub(total_tokens) / 1000
    ))
}

/// The boundary a proactive threshold check runs from. The mid-run
/// boundaries share one failure policy: log and continue, leaving an
/// oversized request to the reactive path, so a failed summarization never
/// ends an in-flight turn.
enum ProactiveTrigger {
    /// A user-visible kickoff started a new run.
    TurnBoundary,
    /// Tool responses landed mid-run.
    MidTurn,
    /// A steer drained after tool responses.
    Steer,
    /// An agent-only continuation appended its message: a stop-hook
    /// denial, or a goal/grind nudge.
    Continuation,
}

impl ProactiveTrigger {
    fn label(&self) -> &'static str {
        match self {
            ProactiveTrigger::TurnBoundary => "turn-boundary",
            ProactiveTrigger::MidTurn => "mid-turn",
            ProactiveTrigger::Steer => "steer",
            ProactiveTrigger::Continuation => "continuation",
        }
    }

    fn continues_run(&self) -> bool {
        !matches!(self, ProactiveTrigger::TurnBoundary)
    }
}

pub struct CompactionOperation {
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
    context_limit: usize,
    threshold: f64,
    manages_own_context: bool,
}

impl CompactionOperation {
    pub fn new(
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        context_limit: usize,
        threshold: f64,
    ) -> Self {
        let manages_own_context = provider.manages_own_context();
        Self {
            provider,
            model_config,
            context_limit,
            threshold,
            manages_own_context,
        }
    }

    fn over_threshold(&self, tokens: usize) -> bool {
        if self.threshold <= 0.0 || self.threshold >= 1.0 {
            return false;
        }
        (tokens as f64 / self.context_limit as f64) > self.threshold
    }

    async fn context_tokens(
        &self,
        session: &Session,
        conversation: &Conversation,
        recount: bool,
    ) -> Result<i32> {
        match session.usage.total_tokens {
            Some(tokens) if !recount => Ok(tokens),
            Some(tokens) => {
                // The metadata covers the preceding request, system prompt and
                // tool schemas included, but not what landed after it; the
                // unsent growth is added to it, replaced tool pairs are
                // subtracted, and the conversation count is kept as a floor.
                let counts = crate::context_mgmt::recount_context_tokens(conversation).await?;
                Ok(counts.estimate(tokens.max(0) as usize) as i32)
            }
            None => crate::context_mgmt::count_context_tokens(conversation).await,
        }
    }

    async fn command_error(
        conversation: &Conversation,
        message: String,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let command = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("compact command conversation has no kickoff message"))?;
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

    async fn clear(
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let command = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("clear command conversation has no kickoff message"))?
            .with_visibility(true, false);
        let response = Message::assistant()
            .with_text("Conversation cleared")
            .with_visibility(true, false);
        let command = emit.message(command).await;
        let response = emit.message(response).await;
        yielded_with([
            Conversation::default().into(),
            command.into(),
            response.into(),
        ])
    }
}

#[async_trait]
impl Operation<Session, GooseEffect> for CompactionOperation {
    fn name(&self) -> &'static str {
        "compaction"
    }

    async fn run_command(
        &self,
        command: &SlashCommand<'_>,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        match command.command {
            "clear" => return Self::clear(conversation, emit).await,
            "compact" => {}
            _ => return not_applicable(),
        }

        let span = chat_span(
            self.provider.as_ref(),
            &self.model_config,
            &session.id,
            "compaction",
        );
        let result = match compact_messages(
            self.provider.as_ref(),
            &self.model_config,
            &session.id,
            conversation,
            true,
        )
        .instrument(span.clone())
        .await
        {
            Ok(result) => result,
            Err(error) => {
                span.record("error.type", "compaction_error");
                return Self::command_error(conversation, error.to_string(), emit).await;
            }
        };
        let compacted = result.conversation;
        let usage = result.usage;
        record_chat_usage(&span, &usage);

        let command = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("compact command conversation has no kickoff message"))?
            .with_visibility(true, false);
        let response = Message::assistant()
            .with_text("Compaction complete")
            .with_visibility(true, false);
        emit.message(command).await;
        let response = emit.message(response).await;
        yielded_with([
            GooseEffect::ReplaceConversation {
                conversation: compacted,
                usage: Some(usage),
            },
            response.into(),
        ])
    }

    async fn moim_parts(
        &self,
        session: &Session,
        conversation: &Conversation,
    ) -> Result<Vec<String>> {
        if self.manages_own_context {
            return Ok(Vec::new());
        }
        Ok(compaction_part(
            Some(self.context_tokens(session, conversation, false).await?),
            self.context_limit,
            self.threshold,
        )
        .into_iter()
        .collect())
    }

    async fn run(
        &self,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        if self.manages_own_context {
            return not_applicable();
        }

        let messages = messages_since_kickoff(conversation)?;
        let reactive_context_error = matches!(
            trailing_error(conversation),
            Some(MessageErrorKind::ContextLengthExceeded)
        );

        let mut proactive_trigger: Option<ProactiveTrigger> = None;
        if reactive_context_error {
            let context_errors = messages
                .iter()
                .filter(|message| {
                    message.error_kind() == Some(MessageErrorKind::ContextLengthExceeded)
                        && !message.is_agent_visible()
                })
                .count();
            if context_errors > MAX_CONTEXT_ERROR_COMPACTIONS {
                return not_applicable();
            }
            debug!(
                trigger = "reactive",
                "auto-compaction triggered by a context-length error"
            );
        } else {
            // Tool output accumulates inside a turn, so a user-boundary-only
            // check cannot see a turn that grows from under the threshold to
            // past the context limit without ever returning to the user.
            //
            // `Tool` is the other safe point: it means the responses for the
            // preceding requests have landed. `Assistant` stays excluded —
            // compacting there would hide tool requests whose responses have
            // not arrived yet, orphaning them.
            let role = last_effective_role(messages)?;
            if !matches!(role, EffectiveRole::User | EffectiveRole::Tool) {
                return not_applicable();
            }
            // A steer drained after tool responses appends a `User` message,
            // so it reaches this check as a turn boundary even though the
            // run is mid-flight — matching the legacy loop's dedicated steer
            // re-check site. Agent-only `User` continuations — a stop-hook
            // denial, a goal/grind nudge — never start a run, so a `User`
            // boundary the user cannot see is also an in-flight continuation.
            let trigger = if role == EffectiveRole::Tool {
                ProactiveTrigger::MidTurn
            } else if messages.iter().any(|message| message.metadata.steer) {
                ProactiveTrigger::Steer
            } else if messages
                .last()
                .is_some_and(|message| !message.is_user_visible())
            {
                ProactiveTrigger::Continuation
            } else {
                ProactiveTrigger::TurnBoundary
            };
            // Steers redefine `messages_since_kickoff`, so both guards below
            // scan back to the run's own kickoff.
            let run_messages = messages_since_run_start(conversation)?;
            // A successful final-output response ends the recipe: the recipe
            // operation delivers it later in this same pass. Summarizing
            // between the response and its delivery wastes a request and, on
            // failure, can end the run without the already-completed result.
            // The guard holds at every boundary while the result is still
            // awaiting that delivery — including one a steer created — but
            // not once an assistant message follows the response: a
            // delivered result must not disable the remaining proactive
            // checks of a run a stop-hook denial resumed.
            if RecipeOperation::final_output_awaiting_delivery(run_messages) {
                return not_applicable();
            }
            // A `Tool` boundary means responses have landed, but not
            // necessarily for every request in a parallel batch: specialized
            // operations answer only their own requests, and the siblings
            // wait for later ones. Compacting while a request is unanswered
            // would replace the history and discard it before it executes,
            // so wait until the batch is complete — the legacy loop persists
            // the whole batch before its check.
            if !pending_tool_requests(run_messages).is_empty() {
                return not_applicable();
            }
            // A proactive check never lands on the provider's own response —
            // a User boundary appends the kickoff (or a steer drained after
            // tool responses), a Tool boundary appends responses — so there
            // is always unsent growth to fold into the session baseline.
            let tokens = self.context_tokens(session, conversation, true).await?;
            if tokens <= 0 || !self.over_threshold(tokens as usize) {
                return not_applicable();
            }
            debug!(
                trigger = trigger.label(),
                tokens,
                context_limit = self.context_limit,
                threshold = self.threshold,
                "auto-compaction threshold exceeded"
            );
            proactive_trigger = Some(trigger);
        }

        let conversation_with_hidden_error;
        let conversation = if reactive_context_error {
            let mut messages = conversation.messages().to_vec();
            let Some(last) = messages.last_mut() else {
                return not_applicable();
            };
            last.metadata.agent_visible = false;
            conversation_with_hidden_error = Conversation::new_unvalidated(messages);
            &conversation_with_hidden_error
        } else {
            conversation
        };

        let threshold_percentage = (self.threshold * 100.0) as u32;
        emit.message(Message::assistant().with_system_notification(
            SystemNotificationType::InlineMessage,
            format!(
                "Exceeded auto-compact threshold of {threshold_percentage}%. \
                     Performing auto-compaction..."
            ),
        ))
        .await;
        emit.message(Message::assistant().with_system_notification(
            SystemNotificationType::ThinkingMessage,
            COMPACTION_THINKING_TEXT,
        ))
        .await;

        let span = chat_span(
            self.provider.as_ref(),
            &self.model_config,
            &session.id,
            "compaction",
        );
        match compact_messages(
            self.provider.as_ref(),
            &self.model_config,
            &session.id,
            conversation,
            false,
        )
        .instrument(span.clone())
        .await
        {
            Ok(result) => {
                let compacted = result.conversation;
                let usage = result.usage;
                record_chat_usage(&span, &usage);
                emit.message(Message::assistant().with_system_notification(
                    SystemNotificationType::InlineMessage,
                    "Compaction complete",
                ))
                .await;
                applied([GooseEffect::ReplaceConversation {
                    conversation: compacted,
                    usage: Some(usage),
                }])
            }
            Err(e) => {
                span.record("error.type", "compaction_error");
                match proactive_trigger {
                    // The run can continue: the request that follows may
                    // still fit, and the reactive path owns the outcome if
                    // not. Ending the run here would discard an
                    // already-completed tool round-trip, drop a drained
                    // steer, or terminate behind an internal nudge.
                    Some(trigger) if trigger.continues_run() => {
                        error!(trigger = trigger.label(), "compaction failed: {e}");
                        return not_applicable();
                    }
                    _ => {
                        emit.message(Message::assistant().with_text(format!(
                            "Ran into this error trying to compact: {e}.\n\n\
                             Please try again or create a new session"
                        )))
                        .await;
                        yielded()
                    }
                }
            }
        }
    }
}
