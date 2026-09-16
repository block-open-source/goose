use crate::conversation::message::{Message, MessageContent};
use chrono::Utc;
use rmcp::model::Role;
use uuid::Uuid;

#[derive(Default)]
pub(super) struct LiveTranscript {
    // Consecutive deltas with the same role form one saved transcript entry.
    transcript_entry_being_built: Option<Message>,
    raw_transcript_entries_waiting_to_save: Vec<Message>,
    // Delegation offsets address individual provider deltas, not grouped messages.
    context_waiting_for_main_agent: Vec<ContextDelta>,
    // A delegation before this prior position would resend old context.
    last_delegation_position_ms: Option<u64>,
}

pub(super) enum DelegationContext {
    Stale,
    MissingUserInput,
    Available(String),
}

struct ContextDelta {
    role: Role,
    text: String,
    end_ms: u64,
}

impl LiveTranscript {
    pub(super) fn append(&mut self, role: Role, delta_text: &str, end_ms: u64) -> Option<Message> {
        if delta_text.is_empty() {
            return None;
        }

        self.context_waiting_for_main_agent.push(ContextDelta {
            role: role.clone(),
            text: delta_text.to_string(),
            end_ms,
        });

        if self
            .transcript_entry_being_built
            .as_ref()
            .is_some_and(|message| message.role == role)
        {
            let message = self
                .transcript_entry_being_built
                .as_mut()
                .expect("message exists");
            let [MessageContent::Text(content)] = message.content.as_mut_slice() else {
                unreachable!("Live transcript messages contain one text block");
            };
            content.text.push_str(delta_text);
            return Some(transcript_delta(message, delta_text));
        }

        if let Some(message) = self.transcript_entry_being_built.take() {
            self.raw_transcript_entries_waiting_to_save.push(message);
        }
        let message = transcript_message(role, delta_text.to_string());
        let transcript_delta_to_display = transcript_delta(&message, delta_text);
        self.transcript_entry_being_built = Some(message);
        Some(transcript_delta_to_display)
    }

    pub(super) fn finish_transcript_entry_being_built(&mut self) {
        if let Some(message) = self.transcript_entry_being_built.take() {
            self.raw_transcript_entries_waiting_to_save.push(message);
        }
    }

    pub(super) fn raw_transcript_entries_waiting_to_save(&self) -> &[Message] {
        &self.raw_transcript_entries_waiting_to_save
    }

    pub(super) fn mark_raw_transcript_entries_saved(&mut self) {
        self.raw_transcript_entries_waiting_to_save.clear();
    }

    pub(super) fn context_for_delegation(&self, offset_ms: u64) -> DelegationContext {
        if self
            .last_delegation_position_ms
            .is_some_and(|last_position| offset_ms < last_position)
        {
            return DelegationContext::Stale;
        }

        let deltas = self
            .context_waiting_for_main_agent
            .iter()
            .filter(|delta| delta.end_ms <= offset_ms)
            .collect::<Vec<_>>();
        if !deltas
            .iter()
            .any(|delta| delta.role == Role::User && !delta.text.trim().is_empty())
        {
            return DelegationContext::MissingUserInput;
        }

        DelegationContext::Available(
            format_context(deltas).expect("delegation context contains transcript"),
        )
    }

    pub(super) fn mark_context_sent_to_main_agent_through(&mut self, offset_ms: u64) {
        self.context_waiting_for_main_agent
            .retain(|delta| delta.end_ms > offset_ms);
        self.last_delegation_position_ms = Some(offset_ms);
    }

    pub(super) fn context_waiting_for_main_agent(&self) -> Option<String> {
        format_context(&self.context_waiting_for_main_agent)
    }

    pub(super) fn agent_only_context_message_waiting_to_save(&self) -> Option<Message> {
        self.context_waiting_for_main_agent().map(|context| {
            Message::user()
                .with_id(format!("msg_live_context_{}", Uuid::now_v7()))
                .with_text(context)
                .agent_only()
        })
    }

    pub(super) fn mark_context_saved_for_main_agent(&mut self) {
        self.context_waiting_for_main_agent.clear();
    }
}

fn format_context<'a>(deltas: impl IntoIterator<Item = &'a ContextDelta>) -> Option<String> {
    let mut input = String::from("Live conversation context:\n");
    let mut previous_role = None;
    for delta in deltas {
        if previous_role.as_ref() == Some(&delta.role) {
            input.push_str(&delta.text);
        } else {
            if previous_role.is_some() {
                input.push('\n');
            }
            input.push_str(speaker(&delta.role));
            input.push_str(": ");
            input.push_str(delta.text.trim_start());
            previous_role = Some(delta.role.clone());
        }
    }
    previous_role?;
    Some(input)
}

fn speaker(role: &Role) -> &'static str {
    match role {
        Role::User => "User",
        Role::Assistant => "GPT-Live",
    }
}

fn transcript_message(role: Role, text: String) -> Message {
    Message::new(role, Utc::now().timestamp(), vec![])
        .with_id(format!("msg_live_{}", Uuid::now_v7()))
        .with_text(text)
        .user_only()
}

fn transcript_delta(message: &Message, text: &str) -> Message {
    Message {
        id: message.id.clone(),
        role: message.role.clone(),
        created: message.created,
        content: vec![MessageContent::text(text)],
        metadata: message.metadata.clone(),
    }
}
