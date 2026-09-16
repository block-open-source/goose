use anyhow::{anyhow, Error, Result};
use async_trait::async_trait;
use goose_providers::live_voice_provider::{
    DelegationUpdate, LiveVoiceInputMessage, LiveVoiceProvider, ProviderConnection,
    ProviderConnectionEvent, WebRtcAnswer, WebRtcOffer,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(super) fn provider_channel() -> (
    Arc<FakeLiveVoiceProvider>,
    mpsc::UnboundedReceiver<FakeStartRequest>,
) {
    let (start_tx, start_rx) = mpsc::unbounded_channel();
    (Arc::new(FakeLiveVoiceProvider { start_tx }), start_rx)
}

pub(super) struct FakeLiveVoiceProvider {
    start_tx: mpsc::UnboundedSender<FakeStartRequest>,
}

#[async_trait]
impl LiveVoiceProvider for FakeLiveVoiceProvider {
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
            .map_err(|_| anyhow!("fake provider driver dropped"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("fake provider start response dropped"))?
    }
}

pub(super) struct FakeStartRequest {
    pub(super) offer: WebRtcOffer,
    pub(super) input_messages: Vec<LiveVoiceInputMessage>,
    response_tx: oneshot::Sender<Result<(WebRtcAnswer, Box<dyn ProviderConnection>)>>,
}

impl FakeStartRequest {
    pub(super) fn accept(self, answer: WebRtcAnswer) -> Result<FakeConnectionDriver> {
        let (stop_request_tx, stop_request_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (delegation_update_tx, delegation_update_rx) = mpsc::unbounded_channel();
        self.response_tx
            .send(Ok((
                answer,
                Box::new(FakeProviderConnection {
                    stop_request_tx,
                    event_rx,
                    delegation_update_tx,
                }),
            )))
            .map_err(|_| anyhow!("fake provider start caller dropped"))?;
        Ok(FakeConnectionDriver {
            stop_request_rx,
            event_tx,
            delegation_update_rx,
        })
    }

    pub(super) fn reject(self, message: impl Into<String>) -> Result<()> {
        self.response_tx
            .send(Err(anyhow!(message.into())))
            .map_err(|_| anyhow!("fake provider start caller dropped"))
    }
}

pub(super) struct FakeConnectionDriver {
    stop_request_rx: mpsc::UnboundedReceiver<oneshot::Sender<Result<(), String>>>,
    event_tx: mpsc::UnboundedSender<ProviderConnectionEvent>,
    delegation_update_rx: mpsc::UnboundedReceiver<DelegationUpdate>,
}

impl FakeConnectionDriver {
    pub(super) async fn next_stop_request(
        &mut self,
    ) -> Option<oneshot::Sender<Result<(), String>>> {
        self.stop_request_rx.recv().await
    }

    pub(super) fn send_event(&self, event: ProviderConnectionEvent) -> Result<()> {
        self.event_tx
            .send(event)
            .map_err(|_| anyhow!("fake provider event receiver dropped"))
    }

    pub(super) async fn next_delegation_update(&mut self) -> Option<DelegationUpdate> {
        self.delegation_update_rx.recv().await
    }
}

struct FakeProviderConnection {
    stop_request_tx: mpsc::UnboundedSender<oneshot::Sender<Result<(), String>>>,
    event_rx: mpsc::UnboundedReceiver<ProviderConnectionEvent>,
    delegation_update_tx: mpsc::UnboundedSender<DelegationUpdate>,
}

#[async_trait]
impl ProviderConnection for FakeProviderConnection {
    async fn next_event(&mut self) -> ProviderConnectionEvent {
        self.event_rx
            .recv()
            .await
            .unwrap_or(ProviderConnectionEvent::Failed)
    }

    async fn send_delegation_update(&mut self, update: DelegationUpdate) -> Result<()> {
        self.delegation_update_tx
            .send(update)
            .map_err(|_| anyhow!("fake provider driver dropped"))
    }

    async fn stop(&mut self) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.stop_request_tx
            .send(response_tx)
            .map_err(|_| anyhow!("fake provider driver dropped"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("fake provider stop response dropped"))?
            .map_err(Error::msg)
    }
}
