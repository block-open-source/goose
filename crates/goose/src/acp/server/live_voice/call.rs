use crate::conversation::message::{Message, MessageContent};
use chrono::Utc;
use goose_providers::live_voice_provider::{ProviderConnection, ProviderConnectionEvent};
use rmcp::model::Role;
use std::collections::HashSet;
use uuid::Uuid;

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
    transcript_events: HashSet<String>,
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
            state: LiveVoiceCallState::Live,
            provider_connection,
            transcript: None,
            transcript_events: HashSet::new(),
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
    ) -> Option<(Option<Message>, Message)> {
        if !self.transcript_events.insert(event_id) {
            return None;
        }
        if delta_text.is_empty() {
            return None;
        }

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
        assert_eq!(call.finish_stop(), LiveVoiceCallState::Stopped);
        did_stop.await.unwrap();
    }

    #[test]
    fn transcript_grouping_projects_deltas_and_finalizes_messages() {
        let mut call = LiveVoiceCall::new(
            "test-session".into(),
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection { stopped: None }),
        );
        let first = call
            .observe_transcript("1".into(), Role::User, "hello")
            .unwrap();
        assert!(first.0.is_none());
        let message_id = first.1.id.clone();
        let second = call
            .observe_transcript("2".into(), Role::User, " world")
            .unwrap();
        assert!(second.0.is_none());
        assert_eq!(second.1.id, message_id);
        assert_eq!(second.1.as_concat_text(), " world");
        assert_eq!(
            call.transcript.as_ref().unwrap().as_concat_text(),
            "hello world"
        );

        assert!(
            call.observe_transcript("2".into(), Role::User, " world")
                .is_none()
        );

        let role_change = call
            .observe_transcript("3".into(), Role::Assistant, "hello")
            .unwrap();
        assert_eq!(role_change.0.unwrap().as_concat_text(), "hello world");
        assert_eq!(role_change.1.role, Role::Assistant);
    }
}
