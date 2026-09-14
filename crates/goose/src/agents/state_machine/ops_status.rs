use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use goose_providers::base::Provider;
use goose_providers::model::ModelConfig;

use crate::agents::state_machine::{
    messages_since_kickoff, not_applicable, yielded_with, ConversationEffect, Emitter, GooseEffect,
    Operation, OperationResult, SlashCommand,
};
use crate::conversation::message::Message;
use crate::conversation::Conversation;
use crate::session::Session;

pub struct StatusOperation {
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
}

impl StatusOperation {
    pub fn new(provider: Arc<dyn Provider>, model_config: ModelConfig) -> Self {
        Self {
            provider,
            model_config,
        }
    }
}

#[async_trait]
impl Operation<Session, GooseEffect> for StatusOperation {
    fn name(&self) -> &'static str {
        "status"
    }

    async fn run_command(
        &self,
        command: &SlashCommand<'_>,
        session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        if command.command != "status" {
            return not_applicable();
        }
        let context_limit = crate::context_limit::get_context_limit(
            self.provider.as_ref(),
            &self.model_config.model_name,
        )
        .await?;
        let context_tokens = session.usage.total_tokens.unwrap_or(0);
        let lifetime_tokens = session.accumulated_usage.total_tokens.unwrap_or(0);
        let context_pct = if context_limit > 0 {
            format!(
                "{}%",
                ((context_tokens as f64 / context_limit as f64) * 100.0)
                    .round()
                    .min(100.0) as usize
            )
        } else {
            "N/A".to_string()
        };
        let response = Message::assistant().with_text(format!("**Session status**\n\n- Model: {}\n- Provider: {}\n- Mode: {}\n- Tokens (lifetime): {}\n- Context: {} / {} tokens ({})", self.model_config.model_name, self.provider.get_name(), session.goose_mode, lifetime_tokens, context_tokens, context_limit, context_pct)).with_visibility(true, false);
        let command_message = messages_since_kickoff(conversation)?
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("status command conversation has no kickoff message"))?;
        let message_id = command_message
            .id
            .clone()
            .ok_or_else(|| anyhow!("Persisted slash command message has no id"))?;
        emit.message(command_message.with_visibility(true, false))
            .await;
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
