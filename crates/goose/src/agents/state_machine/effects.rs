use crate::conversation::message::Message;
use crate::conversation::Conversation;
use crate::providers::base::ProviderUsage;
use crate::recipe::Recipe;
use crate::session::ExtensionData;
use goose_agent::operation::{ConversationEffect, MachineEffect};

pub enum GooseEffect {
    Conversation(ConversationEffect),
    CompactConversation {
        conversation: Conversation,
        usage: Option<ProviderUsage>,
    },
    SetRecipe(Box<Option<Recipe>>),
    SetExtensionData(ExtensionData),
    RecordUsage(ProviderUsage),
}

impl MachineEffect for GooseEffect {
    fn ensure_message_ids(&mut self) {
        match self {
            GooseEffect::Conversation(effect) => effect.ensure_message_ids(),
            GooseEffect::CompactConversation { conversation, .. } => {
                for message in conversation.messages_mut() {
                    if message.id.is_none() {
                        message.id = Some(format!("msg_{}", uuid::Uuid::new_v4()));
                    }
                }
            }
            _ => {}
        }
    }
}

impl From<ConversationEffect> for GooseEffect {
    fn from(effect: ConversationEffect) -> Self {
        GooseEffect::Conversation(effect)
    }
}

impl From<Message> for GooseEffect {
    fn from(message: Message) -> Self {
        ConversationEffect::from(message).into()
    }
}

impl From<Conversation> for GooseEffect {
    fn from(conversation: Conversation) -> Self {
        ConversationEffect::from(conversation).into()
    }
}
