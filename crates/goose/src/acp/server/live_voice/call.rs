use goose_providers::live_voice_provider::{ProviderConnection, ProviderConnectionEvent};
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
    id: LiveVoiceCallId,
    state: LiveVoiceCallState,
    provider_connection: Box<dyn ProviderConnection>,
}

impl LiveVoiceCall {
    pub(super) fn new(
        id: LiveVoiceCallId,
        provider_connection: Box<dyn ProviderConnection>,
    ) -> Self {
        Self {
            id,
            state: LiveVoiceCallState::Live,
            provider_connection,
        }
    }

    pub(super) fn id(&self) -> &LiveVoiceCallId {
        &self.id
    }

    pub(super) async fn next_provider_event(&mut self) -> ProviderConnectionEvent {
        self.provider_connection.next_event().await
    }

    pub(super) fn observe_provider_event(
        &mut self,
        event: ProviderConnectionEvent,
    ) -> LiveVoiceCallState {
        self.state = match (self.state, event) {
            (LiveVoiceCallState::Live, _) => LiveVoiceCallState::Failed,
            (terminal, _) => terminal,
        };
        self.state
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

    pub(super) async fn stop(&mut self) -> LiveVoiceCallState {
        if self.state == LiveVoiceCallState::Live {
            self.state = if self.provider_connection.stop().await.is_ok() {
                LiveVoiceCallState::Stopped
            } else {
                LiveVoiceCallState::Failed
            };
        }
        self.state
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
            LiveVoiceCallId("live-test".into()),
            Box::new(TestConnection {
                stopped: Some(stopped),
            }),
        );

        assert_eq!(call.stop().await, LiveVoiceCallState::Stopped);
        did_stop.await.unwrap();
    }

    #[tokio::test]
    async fn provider_events_are_interpreted_through_call_intent() {
        let mut active = LiveVoiceCall::new(
            LiveVoiceCallId("active".into()),
            Box::new(TestConnection { stopped: None }),
        );
        assert_eq!(
            active.observe_provider_event(ProviderConnectionEvent::Closed),
            LiveVoiceCallState::Failed
        );

        let mut failed = LiveVoiceCall::new(
            LiveVoiceCallId("failed".into()),
            Box::new(TestConnection { stopped: None }),
        );
        assert_eq!(
            failed.observe_provider_event(ProviderConnectionEvent::Failed),
            LiveVoiceCallState::Failed
        );
    }
}
