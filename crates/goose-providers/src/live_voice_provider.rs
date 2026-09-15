//! The application-facing boundary for live voice providers.

use anyhow::Result;
use async_trait::async_trait;

const MAX_SDP_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebRtcOffer(String);

impl WebRtcOffer {
    pub fn new(sdp: String) -> Option<Self> {
        valid_sdp(&sdp).then_some(Self(sdp))
    }

    pub fn into_sdp(self) -> String {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebRtcAnswer(String);

impl WebRtcAnswer {
    pub fn new(sdp: String) -> Option<Self> {
        valid_sdp(&sdp).then_some(Self(sdp))
    }

    pub fn into_sdp(self) -> String {
        self.0
    }
}

fn valid_sdp(sdp: &str) -> bool {
    !sdp.is_empty() && sdp.len() <= MAX_SDP_BYTES
}

#[async_trait]
pub trait ProviderConnection: Send {
    async fn next_event(&mut self) -> ProviderConnectionEvent;
    async fn stop(&mut self) -> Result<()>;
}

#[derive(Debug, PartialEq)]
pub enum ProviderConnectionEvent {
    TranscriptDelta {
        event_id: String,
        role: rmcp::model::Role,
        text: String,
    },
    ReceiverLagged,
    Closed,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveVoiceInputMessage {
    pub role: rmcp::model::Role,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveVoiceProviderAvailability {
    Ready,
    Disabled,
    Unavailable,
}

#[async_trait]
pub trait LiveVoiceProvider: Send + Sync {
    fn availability(&self) -> LiveVoiceProviderAvailability;

    async fn start(
        &self,
        offer: WebRtcOffer,
        input_messages: Vec<LiveVoiceInputMessage>,
    ) -> Result<(WebRtcAnswer, Box<dyn ProviderConnection>)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdp_values_must_be_present_and_bounded() {
        assert!(WebRtcOffer::new(String::new()).is_none());
        assert!(WebRtcOffer::new("x".repeat(MAX_SDP_BYTES + 1)).is_none());
        assert!(WebRtcOffer::new("offer".into()).is_some());
        assert!(WebRtcAnswer::new("answer".into()).is_some());
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod fake {
    use super::*;
    use tokio::sync::{mpsc, oneshot};

    pub fn provider_channel() -> (
        std::sync::Arc<FakeLiveVoiceProvider>,
        mpsc::UnboundedReceiver<FakeStartRequest>,
    ) {
        provider_channel_with_availability(LiveVoiceProviderAvailability::Ready)
    }

    pub fn provider_channel_with_availability(
        availability: LiveVoiceProviderAvailability,
    ) -> (
        std::sync::Arc<FakeLiveVoiceProvider>,
        mpsc::UnboundedReceiver<FakeStartRequest>,
    ) {
        let (start_tx, start_rx) = mpsc::unbounded_channel();
        (
            std::sync::Arc::new(FakeLiveVoiceProvider {
                availability,
                start_tx,
            }),
            start_rx,
        )
    }

    pub struct FakeLiveVoiceProvider {
        availability: LiveVoiceProviderAvailability,
        start_tx: mpsc::UnboundedSender<FakeStartRequest>,
    }

    #[async_trait]
    impl LiveVoiceProvider for FakeLiveVoiceProvider {
        fn availability(&self) -> LiveVoiceProviderAvailability {
            self.availability
        }

        async fn start(
            &self,
            offer: WebRtcOffer,
            input_messages: Vec<LiveVoiceInputMessage>,
        ) -> Result<(WebRtcAnswer, Box<dyn ProviderConnection>)> {
            let (response_tx, response_rx) = oneshot::channel();
            self.start_tx
                .send(FakeStartRequest {
                    offer,
                    input_messages,
                    response_tx,
                })
                .map_err(|_| anyhow::anyhow!("fake provider driver dropped"))?;
            response_rx
                .await
                .map_err(|_| anyhow::anyhow!("fake provider start response dropped"))?
        }
    }

    pub struct FakeStartRequest {
        pub offer: WebRtcOffer,
        pub input_messages: Vec<LiveVoiceInputMessage>,
        response_tx: oneshot::Sender<Result<(WebRtcAnswer, Box<dyn ProviderConnection>)>>,
    }

    impl FakeStartRequest {
        pub fn accept(self, answer: WebRtcAnswer) -> Result<FakeConnectionDriver> {
            let (stop_request_tx, stop_request_rx) = mpsc::unbounded_channel();
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            self.response_tx
                .send(Ok((
                    answer,
                    Box::new(FakeProviderConnection {
                        stop_request_tx,
                        event_rx,
                    }),
                )))
                .map_err(|_| anyhow::anyhow!("fake provider start caller dropped"))?;
            Ok(FakeConnectionDriver {
                stop_request_rx,
                event_tx,
            })
        }

        pub fn reject(self, message: impl Into<String>) -> Result<()> {
            self.response_tx
                .send(Err(anyhow::anyhow!(message.into())))
                .map_err(|_| anyhow::anyhow!("fake provider start caller dropped"))
        }
    }

    pub struct FakeConnectionDriver {
        stop_request_rx: mpsc::UnboundedReceiver<oneshot::Sender<Result<(), String>>>,
        event_tx: mpsc::UnboundedSender<ProviderConnectionEvent>,
    }

    impl FakeConnectionDriver {
        pub async fn next_stop_request(&mut self) -> Option<oneshot::Sender<Result<(), String>>> {
            self.stop_request_rx.recv().await
        }

        pub fn send_event(&self, event: ProviderConnectionEvent) -> Result<()> {
            self.event_tx
                .send(event)
                .map_err(|_| anyhow::anyhow!("fake provider event receiver dropped"))
        }
    }

    struct FakeProviderConnection {
        stop_request_tx: mpsc::UnboundedSender<oneshot::Sender<Result<(), String>>>,
        event_rx: mpsc::UnboundedReceiver<ProviderConnectionEvent>,
    }

    #[async_trait]
    impl ProviderConnection for FakeProviderConnection {
        async fn next_event(&mut self) -> ProviderConnectionEvent {
            self.event_rx
                .recv()
                .await
                .unwrap_or(ProviderConnectionEvent::Failed)
        }

        async fn stop(&mut self) -> Result<()> {
            let (response_tx, response_rx) = oneshot::channel();
            self.stop_request_tx
                .send(response_tx)
                .map_err(|_| anyhow::anyhow!("fake provider driver dropped"))?;
            response_rx
                .await
                .map_err(|_| anyhow::anyhow!("fake provider stop response dropped"))?
                .map_err(anyhow::Error::msg)
        }
    }
}
