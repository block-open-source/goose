use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::AgentEvent;
use goose_provider_types::conversation::message::{Message, MessageContent, MessageErrorKind};
use goose_provider_types::conversation::{effective_role, Conversation, EffectiveRole};
use rmcp::model::Tool;

pub type OperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct SlashCommand<'a> {
    pub command: &'a str,
    pub params_str: &'a str,
}

pub fn messages_since_kickoff(conversation: &Conversation) -> Result<&[Message]> {
    let messages = conversation.messages();
    let start = messages
        .iter()
        .rposition(|message| {
            message.role == rmcp::model::Role::User
                && message.is_user_visible()
                && !message.is_tool_response()
        })
        .ok_or_else(|| anyhow!("state machine conversation has no kickoff message"))?;
    Ok(&messages[start..])
}

pub fn trailing_error(conversation: &Conversation) -> Option<MessageErrorKind> {
    conversation.last().and_then(Message::error_kind)
}

pub fn last_effective_role(messages: &[Message]) -> Result<EffectiveRole> {
    messages
        .last()
        .map(effective_role)
        .ok_or_else(|| anyhow!("cannot determine the role of an empty conversation"))
}

pub fn assistant_turn_count(messages: &[Message]) -> u32 {
    let mut turns = 0;
    let mut in_assistant_block = false;
    for message in messages.iter().rev() {
        if message.role == rmcp::model::Role::Assistant {
            if !in_assistant_block {
                turns += 1;
                in_assistant_block = true;
            }
        } else {
            in_assistant_block = false;
        }
    }
    turns
}

pub fn ends_turn(messages: &[Message]) -> bool {
    messages.last().is_some_and(|last| {
        last.role == rmcp::model::Role::Assistant
            && last.error_kind().is_none()
            && !last.content.iter().any(|content| {
                matches!(
                    content,
                    MessageContent::ToolRequest(_) | MessageContent::ActionRequired(_)
                )
            })
    })
}

#[async_trait]
pub trait Operation<S, E: Send + 'static = ConversationEffect>: Send + Sync {
    fn name(&self) -> &'static str;

    /// Note on a message something this operation did, so that a pipeline rebuilt
    /// from the persisted conversation reaches the same conclusion. Notes record
    /// past actions only — anything an operation would have to compute again does
    /// not belong here.
    fn set_message_meta(&self, message: &mut Message, key: &str, value: serde_json::Value) {
        message.metadata.set_operation_note(self.name(), key, value);
    }

    fn message_meta<'a>(&self, message: &'a Message, key: &str) -> Option<&'a serde_json::Value> {
        message.metadata.operation_note(self.name(), key)
    }

    async fn cancel(
        &self,
        _session: &S,
        _conversation: &Conversation,
        result: OperationResult<E>,
        _emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        Ok(result)
    }

    async fn run_command(
        &self,
        _command: &SlashCommand<'_>,
        _session: &S,
        _conversation: &Conversation,
        _emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        not_applicable()
    }

    async fn inference_tools(&self, _session: &S) -> Result<Vec<Tool>> {
        Ok(Vec::new())
    }

    async fn prompt_parts(
        &self,
        _session: &S,
        _conversation: &Conversation,
    ) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    async fn moim_parts(&self, _session: &S, _conversation: &Conversation) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn run(
        &self,
        _session: &S,
        _conversation: &Conversation,
        _emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        not_applicable()
    }
}

#[derive(Default)]
pub struct InferenceInput {
    pub tools: Vec<Tool>,
    pub prompt_parts: Vec<(String, String)>,
    pub moim_parts: Vec<String>,
}

#[async_trait]
pub trait Inference<S, E: Send + 'static = ConversationEffect>: Operation<S, E> {
    /// Whether the next step would reach the provider. The machine asks before
    /// firing the hooks that mark the start of a turn.
    fn applies(&self, conversation: &Conversation) -> bool;

    async fn infer(
        &self,
        session: &S,
        conversation: &Conversation,
        input: InferenceInput,
        emit: &Emitter,
    ) -> Result<OperationResult<E>>;
}

pub struct StepResult<E = ConversationEffect> {
    pub effects: Vec<E>,
    pub applied_step: Option<&'static str>,
    pub yield_to_client: bool,
}

pub enum OperationResult<E = ConversationEffect> {
    NotApplicable,
    Applied(StepResult<E>),
}

pub fn not_applicable<E>() -> Result<OperationResult<E>> {
    Ok(OperationResult::NotApplicable)
}

pub fn applied<E>(effects: impl IntoIterator<Item = E>) -> Result<OperationResult<E>> {
    Ok(OperationResult::Applied(StepResult {
        effects: effects.into_iter().collect(),
        applied_step: None,
        yield_to_client: false,
    }))
}

pub fn yielded<E>() -> Result<OperationResult<E>> {
    Ok(OperationResult::Applied(StepResult {
        effects: Vec::new(),
        applied_step: None,
        yield_to_client: true,
    }))
}

pub fn yielded_with<E>(effects: impl IntoIterator<Item = E>) -> Result<OperationResult<E>> {
    Ok(OperationResult::Applied(StepResult {
        effects: effects.into_iter().collect(),
        applied_step: None,
        yield_to_client: true,
    }))
}

pub trait MachineEffect {
    fn ensure_message_ids(&mut self);
}

pub enum ConversationEffect {
    AppendMessage(Message),
    ReplaceConversation(Conversation),
    PatchToolRequestMeta {
        tool_call_id: String,
        patch: serde_json::Value,
    },
    SetMessageVisibility {
        message_id: String,
        user_visible: bool,
        agent_visible: bool,
    },
}

impl MachineEffect for ConversationEffect {
    fn ensure_message_ids(&mut self) {
        let messages = match self {
            ConversationEffect::AppendMessage(message) => std::slice::from_mut(message),
            ConversationEffect::ReplaceConversation(conversation) => {
                conversation.messages_mut().as_mut_slice()
            }
            _ => return,
        };
        for message in messages {
            if message.id.is_none() {
                message.id = Some(format!("msg_{}", uuid::Uuid::new_v4()));
            }
        }
    }
}

impl From<Message> for ConversationEffect {
    fn from(message: Message) -> Self {
        ConversationEffect::AppendMessage(message)
    }
}

impl From<Conversation> for ConversationEffect {
    fn from(conversation: Conversation) -> Self {
        ConversationEffect::ReplaceConversation(conversation)
    }
}

pub struct Emitter {
    tx: mpsc::Sender<AgentEvent>,
    cancel: CancellationToken,
}

impl Emitter {
    pub fn new(tx: mpsc::Sender<AgentEvent>, cancel: CancellationToken) -> Self {
        Self { tx, cancel }
    }

    pub async fn emit(&self, event: AgentEvent) {
        let _ = self.tx.send(event).await;
    }

    pub async fn message(&self, message: Message) -> Message {
        let message = message.with_generated_id_if_missing();
        self.emit(AgentEvent::Message(message.clone())).await;
        message
    }

    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub async fn cancelled(&self) {
        self.cancel.cancelled().await
    }
}
