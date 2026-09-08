use goose_providers::live_voice_provider::ProviderConnection;
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
    Stopping,
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

    pub(super) async fn stop(&mut self) -> LiveVoiceCallState {
        if matches!(
            self.state,
            LiveVoiceCallState::Live | LiveVoiceCallState::Stopping
        ) {
            self.state = LiveVoiceCallState::Stopping;
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
}
