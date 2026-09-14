use crate::conversation::message::{Message, MessageContent};
use chrono::Utc;
use goose_providers::live_voice_provider::{
    LiveVoiceInputMessage, ProviderConnection, ProviderConnectionEvent,
};
use rmcp::model::Role;
use std::collections::HashSet;
use uuid::Uuid;

const DELEGATION_INSTRUCTION: &str =
    "Based on this conversation, identify and complete the user's request.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveVoiceCallId(pub(super) String);

impl LiveVoiceCallId {
    pub(super) fn new() -> Self {
        Self(format!("live_{}", Uuid::now_v7()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveVoiceCallState {
    Live,
    Stopped,
    Failed,
}

impl LiveVoiceCallState {
    pub(super) fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }
}

pub(super) struct LiveVoiceCall {
    session_id: String,
    id: LiveVoiceCallId,
    state: LiveVoiceCallState,
    provider_connection: Box<dyn ProviderConnection>,
    transcript: Option<Message>,
    provider_events: HashSet<String>,
    transcript_fragments: Vec<TimedTranscriptFragment>,
    delegation_ids: HashSet<String>,
    last_delegation_offset_ms: Option<u64>,
    startup_context: Vec<LiveVoiceInputMessage>,
}

#[derive(Clone, Debug, PartialEq)]
struct TimedTranscriptFragment {
    role: Role,
    text: String,
    start_ms: u64,
    end_ms: u64,
    accepted: bool,
}

pub(super) enum DelegationInput {
    Ignore,
    Reject(String),
    Accept(String),
}

impl LiveVoiceCall {
    pub(super) fn new(
        session_id: String,
        id: LiveVoiceCallId,
        provider_connection: Box<dyn ProviderConnection>,
        startup_context: Vec<LiveVoiceInputMessage>,
    ) -> Self {
        Self {
            session_id,
            id,
            state: LiveVoiceCallState::Live,
            provider_connection,
            transcript: None,
            provider_events: HashSet::new(),
            transcript_fragments: Vec::new(),
            delegation_ids: HashSet::new(),
            last_delegation_offset_ms: None,
            startup_context,
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
        start_ms: u64,
        end_ms: u64,
    ) -> Option<(Option<Message>, Message)> {
        if !self.provider_events.insert(event_id) {
            return None;
        }
        if delta_text.is_empty() {
            return None;
        }

        self.transcript_fragments.push(TimedTranscriptFragment {
            role: role.clone(),
            text: delta_text.to_string(),
            start_ms,
            end_ms,
            accepted: false,
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
    ) -> DelegationInput {
        if !self.provider_events.insert(event_id) || !self.delegation_ids.insert(delegation_id) {
            return DelegationInput::Ignore;
        }
        if self
            .last_delegation_offset_ms
            .is_some_and(|last_offset| offset_ms < last_offset)
        {
            return DelegationInput::Reject("The delegated conversation position is stale.".into());
        }

        let fragment_indexes = self
            .transcript_fragments
            .iter()
            .enumerate()
            .filter(|(_, fragment)| {
                !fragment.accepted && fragment.start_ms <= offset_ms && fragment.end_ms <= offset_ms
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if !fragment_indexes.iter().any(|index| {
            let fragment = &self.transcript_fragments[*index];
            fragment.role == Role::User && !fragment.text.trim().is_empty()
        }) {
            return DelegationInput::Reject("I couldn't identify a request to complete.".into());
        }

        let mut input = String::from("Live conversation context:\n");
        if self.last_delegation_offset_ms.is_none() {
            for message in &self.startup_context {
                input.push_str(&format!("{}: {}\n", speaker(&message.role), message.text));
            }
        }
        let mut previous_role = None;
        for index in &fragment_indexes {
            let fragment = &self.transcript_fragments[*index];
            if previous_role == Some(&fragment.role) {
                input.push_str(&fragment.text);
            } else {
                if previous_role.is_some() {
                    input.push('\n');
                }
                input.push_str(speaker(&fragment.role));
                input.push_str(": ");
                input.push_str(fragment.text.trim_start());
                previous_role = Some(&fragment.role);
            }
        }
        if previous_role.is_some() {
            input.push('\n');
        }
        input.push_str(DELEGATION_INSTRUCTION);
        for index in fragment_indexes {
            self.transcript_fragments[index].accepted = true;
        }
        self.last_delegation_offset_ms = Some(offset_ms);
        DelegationInput::Accept(input)
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

    pub(super) fn fail(&mut self) -> LiveVoiceCallState {
        if !self.state.is_terminal() {
            self.state = LiveVoiceCallState::Failed;
        }
        self.state
    }

    pub(super) fn finish_stop(&mut self) -> LiveVoiceCallState {
        if !self.state.is_terminal() {
            self.state = LiveVoiceCallState::Stopped;
        }
        self.state
    }
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
            Vec::new(),
        );

        call.cleanup_provider().await.unwrap();
        assert_eq!(call.finish_stop(), LiveVoiceCallState::Stopped);
        did_stop.await.unwrap();
    }

    #[test]
    fn delegation_input_respects_offset_and_suppresses_duplicates() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
            vec![LiveVoiceInputMessage {
                role: Role::User,
                text: "prior context".into(),
            }],
        );
        call.observe_transcript("1".into(), Role::Assistant, "ready", 0, 5);
        call.observe_transcript("2".into(), Role::User, "do ", 5, 10);
        call.observe_transcript("3".into(), Role::User, "this", 10, 20);
        call.observe_transcript("4".into(), Role::Assistant, "crossing", 15, 25);
        call.observe_transcript("5".into(), Role::Assistant, "later", 30, 40);
        let open_transcript = call.transcript.as_ref().unwrap().as_concat_text();

        let DelegationInput::Accept(input) =
            call.delegation_input("event-1".into(), "delegation-1".into(), 20)
        else {
            panic!("delegation should be accepted");
        };
        assert!(input.contains("prior context"));
        assert!(input.contains("User: do this\n"));
        assert!(!input.contains("crossing"));
        assert!(!input.contains("later"));
        assert_eq!(
            call.transcript.as_ref().unwrap().as_concat_text(),
            open_transcript
        );
        assert!(matches!(
            call.delegation_input("event-1".into(), "delegation-1".into(), 20),
            DelegationInput::Ignore
        ));

        call.observe_transcript("6".into(), Role::User, " late detail", 15, 20);
        let DelegationInput::Accept(continuation) =
            call.delegation_input("event-2".into(), "delegation-2".into(), 20)
        else {
            panic!("continuation should be accepted");
        };
        assert!(continuation.contains("User: late detail\n"));
        assert!(!continuation.contains("prior context"));
        assert!(!continuation.contains("do this"));

        assert!(matches!(
            call.delegation_input("event-3".into(), "delegation-3".into(), 19),
            DelegationInput::Reject(_)
        ));

        call.observe_transcript("7".into(), Role::User, "again", 40, 50);
        let DelegationInput::Accept(next) =
            call.delegation_input("event-4".into(), "delegation-4".into(), 50)
        else {
            panic!("later continuation should be accepted");
        };
        assert!(next.contains("GPT-Live: crossinglater\nUser: again\n"));
        assert!(!next.contains("prior context"));
        assert!(!next.contains("late detail"));

        let mut missing_user = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test-2".into()),
            Box::new(TestConnection { stopped: None }),
            Vec::new(),
        );
        missing_user.observe_transcript("1".into(), Role::Assistant, "hello", 0, 10);
        assert!(matches!(
            missing_user.delegation_input("event-1".into(), "delegation-1".into(), 10),
            DelegationInput::Reject(_)
        ));
    }

    #[test]
    fn transcript_grouping_projects_deltas_and_finalizes_messages() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
            Vec::new(),
        );
        let first = call
            .observe_transcript("1".into(), Role::User, "hello", 0, 10)
            .unwrap();
        assert!(first.0.is_none());
        let message_id = first.1.id.clone();
        let second = call
            .observe_transcript("2".into(), Role::User, " world", 10, 20)
            .unwrap();
        assert!(second.0.is_none());
        assert_eq!(second.1.id, message_id);
        assert_eq!(second.1.as_concat_text(), " world");
        assert_eq!(
            call.transcript.as_ref().unwrap().as_concat_text(),
            "hello world"
        );

        assert!(call
            .observe_transcript("2".into(), Role::User, " world", 10, 20)
            .is_none());

        let role_change = call
            .observe_transcript("3".into(), Role::Assistant, "hello", 20, 30)
            .unwrap();
        assert_eq!(role_change.0.unwrap().as_concat_text(), "hello world");
        assert_eq!(role_change.1.role, Role::Assistant);
    }
}
