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
    async fn send_delegation_update(&mut self, update: DelegationUpdate) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
}

#[derive(Debug, PartialEq)]
pub enum ProviderConnectionEvent {
    TranscriptDelta {
        event_id: String,
        role: rmcp::model::Role,
        text: String,
        start_ms: u64,
        end_ms: u64,
    },
    DelegationRequested {
        event_id: String,
        delegation_id: String,
        offset_ms: u64,
    },
    ReceiverLagged,
    Closed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationUpdate {
    pub provider_delegation_id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveVoiceInputMessage {
    pub role: rmcp::model::Role,
    pub text: String,
}

#[async_trait]
pub trait LiveVoiceProvider: Send + Sync {
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
