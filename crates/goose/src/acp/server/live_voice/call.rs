use crate::conversation::message::{Message, MessageContent};
use chrono::Utc;
use goose_providers::live_voice_provider::{ProviderConnection, ProviderConnectionEvent};
use rmcp::model::Role;
use std::collections::HashSet;
use uuid::Uuid;

pub(super) const DELEGATION_INSTRUCTION: &str =
    "Based on this conversation, identify and complete the user's request.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveVoiceCallId(pub(super) String);

impl LiveVoiceCallId {
    pub(super) fn new() -> Self {
        Self(format!("live_{}", Uuid::now_v7()))
    }
}

pub(super) struct LiveVoiceCall {
    session_id: String,
    id: LiveVoiceCallId,
    provider_connection: Box<dyn ProviderConnection>,
    transcript: Option<Message>,
    provider_events: HashSet<String>,
    transcript_fragments: Vec<TranscriptFragment>,
    delegation_ids: HashSet<String>,
    last_delegation_offset_ms: Option<u64>,
}

struct TranscriptFragment {
    role: Role,
    text: String,
    end_ms: u64,
}

pub(super) enum DelegationDecision {
    Ignore,
    Reject(String),
    Accept(String),
}

impl LiveVoiceCall {
    pub(super) fn new(
        session_id: String,
        id: LiveVoiceCallId,
        provider_connection: Box<dyn ProviderConnection>,
    ) -> Self {
        Self {
            session_id,
            id,
            provider_connection,
            transcript: None,
            provider_events: HashSet::new(),
            transcript_fragments: Vec::new(),
            delegation_ids: HashSet::new(),
            last_delegation_offset_ms: None,
        }
    }

    pub(super) fn id(&self) -> &LiveVoiceCallId {
        &self.id
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(super) async fn next_provider_event(&mut self) -> ProviderConnectionEvent {
        self.provider_connection.next_event().await
    }

    pub(super) fn observe_transcript(
        &mut self,
        event_id: String,
        role: Role,
        delta_text: &str,
        end_ms: u64,
    ) -> Option<(Option<Message>, Message)> {
        if !self.provider_events.insert(event_id) {
            return None;
        }
        if delta_text.is_empty() {
            return None;
        }

        self.transcript_fragments.push(TranscriptFragment {
            role: role.clone(),
            text: delta_text.to_string(),
            end_ms,
        });

        if self
            .transcript
            .as_ref()
            .is_some_and(|message| message.role == role)
        {
            let message = self.transcript.as_mut().expect("transcript exists");
            let [MessageContent::Text(content)] = message.content.as_mut_slice() else {
                unreachable!("Live transcript messages contain one text block");
            };
            content.text.push_str(delta_text);
            return Some((None, transcript_delta_message(message, delta_text)));
        }

        let finalized = self.transcript.take();
        let message = live_transcript_message(role, delta_text.to_string());
        let delta = transcript_delta_message(&message, delta_text);
        self.transcript = Some(message);
        Some((finalized, delta))
    }

    pub(super) fn take_transcript(&mut self) -> Option<Message> {
        self.transcript.take()
    }

    pub(super) fn delegation_input(
        &mut self,
        event_id: String,
        delegation_id: String,
        offset_ms: u64,
    ) -> DelegationDecision {
        if !self.provider_events.insert(event_id) || !self.delegation_ids.insert(delegation_id) {
            return DelegationDecision::Ignore;
        }
        if self
            .last_delegation_offset_ms
            .is_some_and(|last_offset| offset_ms < last_offset)
        {
            return DelegationDecision::Reject(
                "The delegated conversation position is stale.".into(),
            );
        }

        let fragments = self
            .transcript_fragments
            .iter()
            .filter(|fragment| fragment.end_ms <= offset_ms)
            .collect::<Vec<_>>();
        if !fragments
            .iter()
            .any(|fragment| fragment.role == Role::User && !fragment.text.trim().is_empty())
        {
            return DelegationDecision::Reject("I couldn't identify a request to complete.".into());
        }

        let input = format_transcript(fragments, Some(DELEGATION_INSTRUCTION))
            .expect("accepted delegation contains transcript");
        DelegationDecision::Accept(input)
    }

    pub(super) fn accept_delegation(&mut self, offset_ms: u64) {
        self.transcript_fragments
            .retain(|fragment| fragment.end_ms > offset_ms);
        self.last_delegation_offset_ms = Some(offset_ms);
    }

    pub(super) fn clear_pending_transcript(&mut self) {
        self.transcript_fragments.clear();
    }

    pub(super) fn pending_transcript_context(&self) -> Option<String> {
        format_transcript(&self.transcript_fragments, None)
    }

    pub(super) async fn send_delegation_update(
        &mut self,
        provider_delegation_id: String,
        text: String,
    ) -> anyhow::Result<()> {
        self.provider_connection
            .send_delegation_update(goose_providers::live_voice_provider::DelegationUpdate {
                provider_delegation_id,
                text,
            })
            .await
    }

    pub(super) async fn cleanup_provider(&mut self) -> anyhow::Result<()> {
        self.provider_connection.stop().await
    }
}

fn format_transcript<'a>(
    fragments: impl IntoIterator<Item = &'a TranscriptFragment>,
    instruction: Option<&str>,
) -> Option<String> {
    let mut input = String::from("Live conversation context:\n");
    let mut previous_role = None;
    for fragment in fragments {
        if previous_role.as_ref() == Some(&fragment.role) {
            input.push_str(&fragment.text);
        } else {
            if previous_role.is_some() {
                input.push('\n');
            }
            input.push_str(speaker(&fragment.role));
            input.push_str(": ");
            input.push_str(fragment.text.trim_start());
            previous_role = Some(fragment.role.clone());
        }
    }
    previous_role?;
    if let Some(instruction) = instruction {
        input.push('\n');
        input.push_str(instruction);
    }
    Some(input)
}

fn speaker(role: &Role) -> &'static str {
    match role {
        Role::User => "User",
        Role::Assistant => "GPT-Live",
    }
}

fn live_transcript_message(role: Role, text: String) -> Message {
    Message::new(role, Utc::now().timestamp(), vec![])
        .with_id(format!("msg_live_{}", Uuid::now_v7()))
        .with_text(text)
}

fn transcript_delta_message(message: &Message, text: &str) -> Message {
    Message {
        id: message.id.clone(),
        role: message.role.clone(),
        created: message.created,
        content: vec![MessageContent::text(text)],
        metadata: message.metadata.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::sync::oneshot;

    struct TestConnection {
        stopped: Option<oneshot::Sender<()>>,
    }

    #[async_trait]
    impl ProviderConnection for TestConnection {
        async fn next_event(&mut self) -> ProviderConnectionEvent {
            std::future::pending().await
        }

        async fn send_delegation_update(
            &mut self,
            _update: goose_providers::live_voice_provider::DelegationUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn stop(&mut self) -> anyhow::Result<()> {
            if let Some(stopped) = self.stopped.take() {
                let _ = stopped.send(());
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_call_owns_and_stops_its_provider_connection() {
        let (stopped, did_stop) = oneshot::channel();
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection {
                stopped: Some(stopped),
            }),
        );

        call.cleanup_provider().await.unwrap();
        did_stop.await.unwrap();
    }

    #[test]
    fn delegation_input_respects_offset_and_suppresses_duplicates() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
        );
        call.observe_transcript("1".into(), Role::Assistant, "ready", 5);
        call.observe_transcript("2".into(), Role::User, "do ", 10);
        call.observe_transcript("3".into(), Role::User, "this", 20);
        call.observe_transcript("4".into(), Role::Assistant, "crossing", 25);
        call.observe_transcript("5".into(), Role::Assistant, "later", 40);
        let open_transcript = call.transcript.as_ref().unwrap().as_concat_text();

        let DelegationDecision::Accept(input) =
            call.delegation_input("event-1".into(), "delegation-1".into(), 20)
        else {
            panic!("delegation should be accepted");
        };
        assert!(input.contains("GPT-Live: ready\nUser: do this\n"));
        assert!(!input.contains("crossing"));
        assert!(!input.contains("later"));
        call.accept_delegation(20);
        assert_eq!(
            call.transcript.as_ref().unwrap().as_concat_text(),
            open_transcript
        );
        assert!(matches!(
            call.delegation_input("event-1".into(), "delegation-1".into(), 20),
            DelegationDecision::Ignore
        ));

        call.observe_transcript("6".into(), Role::User, " late detail", 20);
        let DelegationDecision::Accept(continuation) =
            call.delegation_input("event-2".into(), "delegation-2".into(), 20)
        else {
            panic!("continuation should be accepted");
        };
        assert!(continuation.contains("User: late detail\n"));
        assert!(!continuation.contains("ready"));
        assert!(!continuation.contains("do this"));
        call.accept_delegation(20);

        assert!(matches!(
            call.delegation_input("event-3".into(), "delegation-3".into(), 19),
            DelegationDecision::Reject(_)
        ));

        call.observe_transcript("7".into(), Role::User, "again", 50);
        let DelegationDecision::Accept(next) =
            call.delegation_input("event-4".into(), "delegation-4".into(), 50)
        else {
            panic!("later continuation should be accepted");
        };
        assert!(next.contains("GPT-Live: crossinglater\nUser: again\n"));
        assert!(!next.contains("ready"));
        assert!(!next.contains("late detail"));
        call.accept_delegation(50);

        let mut missing_user = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test-2".into()),
            Box::new(TestConnection { stopped: None }),
        );
        missing_user.observe_transcript("1".into(), Role::Assistant, "hello", 10);
        assert!(matches!(
            missing_user.delegation_input("event-1".into(), "delegation-1".into(), 10),
            DelegationDecision::Reject(_)
        ));
    }

    #[test]
    fn pending_context_has_no_action_instruction_and_can_be_cleared() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
        );
        call.observe_transcript("1".into(), Role::Assistant, "Anything else?", 10);
        call.observe_transcript("2".into(), Role::User, "No thanks", 20);

        let context = call.pending_transcript_context().unwrap();
        assert_eq!(
            context,
            "Live conversation context:\nGPT-Live: Anything else?\nUser: No thanks"
        );
        assert!(!context.contains(DELEGATION_INSTRUCTION));
        assert_eq!(call.pending_transcript_context().unwrap(), context);

        call.observe_transcript("3".into(), Role::User, "Already shared", 30);
        call.clear_pending_transcript();
        assert!(call.pending_transcript_context().is_none());
    }

    #[test]
    fn transcript_grouping_projects_deltas_and_finalizes_messages() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
        );
        let first = call
            .observe_transcript("1".into(), Role::User, "hello", 10)
            .unwrap();
        assert!(first.0.is_none());
        assert!(first.1.is_user_visible());
        assert!(first.1.is_agent_visible());
        let message_id = first.1.id.clone();
        let second = call
            .observe_transcript("2".into(), Role::User, " world", 20)
            .unwrap();
        assert!(second.0.is_none());
        assert_eq!(second.1.id, message_id);
        assert_eq!(second.1.as_concat_text(), " world");
        assert_eq!(
            call.transcript.as_ref().unwrap().as_concat_text(),
            "hello world"
        );

        assert!(call
            .observe_transcript("2".into(), Role::User, " world", 20)
            .is_none());

        let role_change = call
            .observe_transcript("3".into(), Role::Assistant, "hello", 30)
            .unwrap();
        assert_eq!(role_change.0.unwrap().as_concat_text(), "hello world");
        assert_eq!(role_change.1.role, Role::Assistant);
    }
}
