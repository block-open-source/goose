//! OpenAI implementation of the live voice provider boundary.

use crate::{
    live::{LiveSessionEndReason, LiveSessionEvent},
    live_voice_provider::{
        LiveVoiceInputMessage, LiveVoiceProvider, LiveVoiceProviderAvailability,
        ProviderConnection, ProviderConnectionEvent, WebRtcAnswer, WebRtcOffer,
    },
    openai_live::{
        ConnectedOpenAiLiveSession, OpenAiLiveClient, OpenAiLiveEvent, OpenAiLiveEventKind,
        OpenAiLiveMessage, OpenAiLiveMessageRole, OpenAiLiveSessionConfig, OpenAiLiveSessionId,
    },
};
use anyhow::{bail, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::{
    sync::broadcast::error::RecvError,
    time::{sleep_until, timeout, timeout_at, Instant},
};

pub const OPENAI_LIVE_VOICE_GATE_ENV: &str = "GOOSE_LIVE_VOICE_ENABLED";
pub const OPENAI_LIVE_MODEL_ENV: &str = "GOOSE_LIVE_VOICE_MODEL";
pub const OPENAI_LIVE_VOICE_ENV: &str = "GOOSE_LIVE_VOICE";
pub const OPENAI_LIVE_API_KEY_ENV: &str = "OPENAI_API_KEY";
pub const DEFAULT_OPENAI_LIVE_MODEL: &str = "gpt-live-1";
pub const DEFAULT_OPENAI_LIVE_VOICE: &str = "marin";

const HTTP_SETUP_TIMEOUT: Duration = Duration::from_secs(15);
const SIDEBAND_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiLiveVoiceConfig {
    pub enabled: bool,
    pub model: String,
    pub voice: String,
}

impl OpenAiLiveVoiceConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var(OPENAI_LIVE_VOICE_GATE_ENV)
                .is_ok_and(|value| value.eq_ignore_ascii_case("true") || value == "1"),
            model: std::env::var(OPENAI_LIVE_MODEL_ENV)
                .unwrap_or_else(|_| DEFAULT_OPENAI_LIVE_MODEL.into()),
            voice: std::env::var(OPENAI_LIVE_VOICE_ENV)
                .unwrap_or_else(|_| DEFAULT_OPENAI_LIVE_VOICE.into()),
        }
    }
}

pub struct OpenAiLiveVoiceProvider {
    client: Option<OpenAiLiveClient>,
    config: OpenAiLiveVoiceConfig,
}

impl OpenAiLiveVoiceProvider {
    pub fn from_env() -> Self {
        let config = OpenAiLiveVoiceConfig::from_env();
        let client = std::env::var(OPENAI_LIVE_API_KEY_ENV)
            .ok()
            .filter(|key| !key.trim().is_empty())
            .map(OpenAiLiveClient::new);
        Self { client, config }
    }

    pub fn new(api_key: impl Into<String>, config: OpenAiLiveVoiceConfig) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            bail!("OpenAI API key is empty");
        }
        Ok(Self {
            client: Some(OpenAiLiveClient::new(api_key)),
            config,
        })
    }
}

#[async_trait]
impl LiveVoiceProvider for OpenAiLiveVoiceProvider {
    fn availability(&self) -> LiveVoiceProviderAvailability {
        if !self.config.enabled {
            LiveVoiceProviderAvailability::Disabled
        } else if self.client.is_none() {
            LiveVoiceProviderAvailability::Unavailable
        } else {
            LiveVoiceProviderAvailability::Ready
        }
    }

    async fn start(
        &self,
        offer: WebRtcOffer,
        input_messages: Vec<LiveVoiceInputMessage>,
    ) -> Result<(WebRtcAnswer, Box<dyn ProviderConnection>)> {
        if !self.config.enabled {
            bail!("OpenAI Live voice is disabled");
        }
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OpenAI Live credentials are unavailable"))?
            .clone();
        let config = OpenAiLiveSessionConfig {
            model: self.config.model.clone(),
            instructions: String::new(),
            voice: Some(self.config.voice.clone()),
            input_messages: input_messages
                .into_iter()
                .map(|message| OpenAiLiveMessage {
                    role: match message.role {
                        rmcp::model::Role::User => OpenAiLiveMessageRole::User,
                        rmcp::model::Role::Assistant => OpenAiLiveMessageRole::Assistant,
                    },
                    text: message.text,
                })
                .collect(),
            extra_session_fields: Default::default(),
        };
        let negotiation = timeout(
            HTTP_SETUP_TIMEOUT,
            client.webrtc(config).negotiate(offer.into_sdp()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI Live HTTP setup timed out"))??;
        let session_id = negotiation.session_id;
        let answer = WebRtcAnswer::new(negotiation.answer_sdp)
            .ok_or_else(|| anyhow::anyhow!("OpenAI Live returned an invalid WebRTC answer"))?;
        let sideband = connect_sideband(&client, session_id).await?;
        Ok((answer, Box::new(OpenAiProviderConnection { sideband })))
    }
}

struct OpenAiProviderConnection {
    sideband: ConnectedOpenAiLiveSession,
}

#[async_trait]
impl ProviderConnection for OpenAiProviderConnection {
    async fn next_event(&mut self) -> ProviderConnectionEvent {
        loop {
            if let Some(event) = provider_connection_event(self.sideband.recv().await) {
                return event;
            }
        }
    }

    async fn stop(&mut self) -> Result<()> {
        self.sideband.close().await
    }
}

fn provider_connection_event(
    event: std::result::Result<LiveSessionEvent<OpenAiLiveEvent>, RecvError>,
) -> Option<ProviderConnectionEvent> {
    match event {
        Ok(LiveSessionEvent::Message(event)) => match event.kind {
            OpenAiLiveEventKind::TranscriptDelta {
                event_id,
                role,
                delta,
                ..
            } => Some(ProviderConnectionEvent::TranscriptDelta {
                event_id,
                role,
                text: delta,
            }),
            OpenAiLiveEventKind::SessionClosed { .. } => Some(ProviderConnectionEvent::Closed),
            _ => None,
        },
        Ok(LiveSessionEvent::Ended {
            reason: LiveSessionEndReason::Closed,
            error: None,
        }) => Some(ProviderConnectionEvent::Closed),
        Err(RecvError::Lagged(_)) => Some(ProviderConnectionEvent::ReceiverLagged),
        Ok(LiveSessionEvent::Ended { .. }) | Err(RecvError::Closed) => {
            Some(ProviderConnectionEvent::Failed)
        }
    }
}

async fn connect_sideband(
    client: &OpenAiLiveClient,
    session_id: OpenAiLiveSessionId,
) -> Result<ConnectedOpenAiLiveSession> {
    let deadline = Instant::now() + SIDEBAND_ATTACH_TIMEOUT;
    loop {
        match timeout_at(
            deadline,
            client.existing_session(session_id.clone()).connect(),
        )
        .await
        {
            Ok(Ok(sideband)) => return Ok(sideband),
            Ok(Err(error)) => {
                let retry_at = Instant::now() + Duration::from_millis(200);
                if retry_at >= deadline {
                    return Err(error);
                }
                sleep_until(retry_at).await;
            }
            Err(_) => bail!("OpenAI Live sideband attachment timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_gated() {
        let provider = OpenAiLiveVoiceProvider::new(
            "key",
            OpenAiLiveVoiceConfig {
                enabled: false,
                model: DEFAULT_OPENAI_LIVE_MODEL.into(),
                voice: DEFAULT_OPENAI_LIVE_VOICE.into(),
            },
        )
        .unwrap();
        assert_eq!(
            provider.availability(),
            LiveVoiceProviderAvailability::Disabled
        );
    }

    #[test]
    fn maps_live_voice_observations() {
        let message = |kind| {
            Ok(LiveSessionEvent::Message(OpenAiLiveEvent {
                kind,
                raw: None,
            }))
        };
        let cases = [
            (
                message(OpenAiLiveEventKind::TranscriptDelta {
                    event_id: "event_transcript_1".into(),
                    client_event_id: None,
                    role: rmcp::model::Role::Assistant,
                    delta: "hello".into(),
                    start_ms: 10,
                    end_ms: 20,
                }),
                Some(ProviderConnectionEvent::TranscriptDelta {
                    event_id: "event_transcript_1".into(),
                    role: rmcp::model::Role::Assistant,
                    text: "hello".into(),
                }),
            ),
            (
                message(OpenAiLiveEventKind::SessionClosed {
                    event_id: "event_closed_1".into(),
                    client_event_id: Some("client_close_1".into()),
                    reason: "close_requested".into(),
                    session: serde_json::json!({ "id": "session_1" }),
                    usage: serde_json::json!({ "seconds": 1 }),
                }),
                Some(ProviderConnectionEvent::Closed),
            ),
            (
                message(OpenAiLiveEventKind::Error {
                    event_id: "event_error_1".into(),
                    error_type: "server_error".into(),
                    code: "provider_failed".into(),
                    message: "provider failed".into(),
                    parameter: None,
                    client_event_id: None,
                }),
                None,
            ),
            (
                Ok(LiveSessionEvent::Ended {
                    reason: LiveSessionEndReason::Closed,
                    error: None,
                }),
                Some(ProviderConnectionEvent::Closed),
            ),
            (
                Ok(LiveSessionEvent::Ended {
                    reason: LiveSessionEndReason::TransportFailed,
                    error: None,
                }),
                Some(ProviderConnectionEvent::Failed),
            ),
            (
                Err(RecvError::Lagged(1)),
                Some(ProviderConnectionEvent::ReceiverLagged),
            ),
            (
                Err(RecvError::Closed),
                Some(ProviderConnectionEvent::Failed),
            ),
            (
                message(OpenAiLiveEventKind::InputMuted {
                    event_id: "event_muted_1".into(),
                    client_event_id: None,
                }),
                None,
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(provider_connection_event(event), expected);
        }
    }
}
