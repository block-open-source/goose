//! Ends the turn when the agent has used its autonomous turn budget.

use crate::agents::state_machine::effects::GooseEffect;
use crate::agents::state_machine::{
    assistant_turn_count, messages_since_kickoff, not_applicable, yielded_with, Emitter, Operation,
    OperationResult,
};
use crate::conversation::message::Message;
use crate::conversation::Conversation;
use crate::session::Session;
use anyhow::Result;
use async_trait::async_trait;

pub const MAX_TURNS_MESSAGE: &str = "I've reached the maximum number of actions I can do without user input. Would you like me to continue?";

pub struct MaxTurnsOperation {
    max_turns: u32,
}

fn turn_budget_part(turns_taken: u32, max_turns: u32) -> Option<String> {
    if max_turns == 0 || turns_taken.saturating_mul(2) < max_turns {
        return None;
    }

    Some(format!(
        "<turn-budget>{turns_taken}/{max_turns} used</turn-budget>"
    ))
}

fn inference_turn_count(messages: &[Message]) -> u32 {
    let messages = messages
        .iter()
        .filter(|message| {
            message
                .metadata
                .operation_note("compaction", "synthetic_turn")
                .is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    assistant_turn_count(&messages)
}

impl MaxTurnsOperation {
    pub fn new(max_turns: u32) -> Self {
        Self { max_turns }
    }
}

#[async_trait]
impl Operation<Session, GooseEffect> for MaxTurnsOperation {
    fn name(&self) -> &'static str {
        "max_turns"
    }

    async fn moim_parts(
        &self,
        _session: &Session,
        conversation: &Conversation,
    ) -> Result<Vec<String>> {
        let turns_taken = inference_turn_count(messages_since_kickoff(conversation)?);
        Ok(turn_budget_part(turns_taken, self.max_turns)
            .into_iter()
            .collect())
    }

    async fn run(
        &self,
        _session: &Session,
        conversation: &Conversation,
        emit: &Emitter,
    ) -> Result<OperationResult<GooseEffect>> {
        let messages = messages_since_kickoff(conversation)?;
        if inference_turn_count(messages) < self.max_turns {
            return not_applicable();
        }

        let message = Message::assistant().with_text(MAX_TURNS_MESSAGE);
        let message = emit.message(message).await;
        yielded_with([message.into()])
    }
}
