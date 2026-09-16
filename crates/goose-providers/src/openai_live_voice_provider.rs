//! OpenAI implementation of the live voice provider boundary.

use crate::{
    live::{LiveSessionEndReason, LiveSessionEvent},
    live_voice_provider::{
        DelegationUpdate, LiveVoiceInputMessage, LiveVoiceProvider, ProviderConnection,
        ProviderConnectionEvent, WebRtcAnswer, WebRtcOffer,
    },
    openai_live::{
        ConnectedOpenAiLiveSession, OpenAiLiveClient, OpenAiLiveContext, OpenAiLiveContextChannel,
        OpenAiLiveDelegationId, OpenAiLiveEvent, OpenAiLiveEventKind, OpenAiLiveMessage,
        OpenAiLiveMessageRole, OpenAiLiveSessionConfig, OpenAiLiveSessionId,
    },
};
use anyhow::{bail, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::{
    sync::broadcast::error::RecvError,
    time::{sleep_until, timeout, timeout_at, Instant},
};

const OPENAI_LIVE_MODEL: &str = "gpt-live-1";
const DEFAULT_OPENAI_LIVE_VOICE: &str = "marin";

const HTTP_SETUP_TIMEOUT: Duration = Duration::from_secs(15);
const SIDEBAND_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);
const LIVE_SESSION_INSTRUCTIONS: &str = concat!(
    "You are Goose's live voice interface. Keep the conversation natural and concise.\n",
    "Interruption policy: Stop speaking when the user interrupts and listen to what they say.\n",
    "Delegation policy:\n",
    "Backend tools:\n",
    "- Goose can use backend reasoning and tools for longer tasks.\n",
    "Delegate to Goose when:\n",
    "- The user has finished stating a complete request that needs backend tools or reasoning.\n",
    "- The user corrects or changes backend work already in progress.\n",
    "Do not delegate to Goose when:\n",
    "- The request is unfinished or is missing a required detail such as a location, object, ",
    "command, or desired outcome. Ask one brief clarification and wait for the answer.\n",
    "- The user is greeting you or making conversation that you can answer directly.\n",
    "After delegating, briefly say the work is underway. Keep listening and accept corrections ",
    "while Goose works. Do not guess the result. Present delegated results directly. Only say ",
    "the task stopped or finished after Goose confirms it."
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiLiveVoiceConfig {
    pub voice: String,
}

impl Default for OpenAiLiveVoiceConfig {
    fn default() -> Self {
        Self {
            voice: DEFAULT_OPENAI_LIVE_VOICE.into(),
        }
    }
}

pub struct OpenAiLiveVoiceProvider {
    client: OpenAiLiveClient,
    config: OpenAiLiveVoiceConfig,
}

impl OpenAiLiveVoiceProvider {
    pub fn new(api_key: impl Into<String>, config: OpenAiLiveVoiceConfig) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            bail!("OpenAI API key is empty");
        }
        if config.voice.trim().is_empty() {
            bail!("OpenAI Live voice is empty");
        }
        Ok(Self {
            client: OpenAiLiveClient::new(api_key),
            config,
        })
    }
}

#[async_trait]
impl LiveVoiceProvider for OpenAiLiveVoiceProvider {
    async fn start(
        &self,
        offer: WebRtcOffer,
        input_messages: Vec<LiveVoiceInputMessage>,
    ) -> Result<(WebRtcAnswer, Box<dyn ProviderConnection>)> {
        let config = OpenAiLiveSessionConfig {
            model: OPENAI_LIVE_MODEL.into(),
            instructions: LIVE_SESSION_INSTRUCTIONS.into(),
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
            self.client.webrtc(config).negotiate(offer.into_sdp()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI Live HTTP setup timed out"))??;
        let session_id = negotiation.session_id;
        let answer = WebRtcAnswer::new(negotiation.answer_sdp)
            .ok_or_else(|| anyhow::anyhow!("OpenAI Live returned an invalid WebRTC answer"))?;
        let sideband = connect_sideband(&self.client, session_id).await?;
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

    async fn send_delegation_update(&mut self, update: DelegationUpdate) -> Result<()> {
        self.sideband
            .send(crate::openai_live::OpenAiLiveCommand::AppendContext {
                event_id: format!("event_{}", uuid::Uuid::new_v4()),
                delegation_id: Some(OpenAiLiveDelegationId(update.provider_delegation_id)),
                context: OpenAiLiveContext {
                    text: update.text,
                    channel: OpenAiLiveContextChannel::Commentary,
                },
            })
            .await
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
                start_ms,
                end_ms,
                ..
            } => Some(ProviderConnectionEvent::TranscriptDelta {
                event_id,
                role,
                text: delta,
                start_ms,
                end_ms,
            }),
            OpenAiLiveEventKind::DelegationCreated {
                event_id,
                delegation,
                ..
            } if matches!(
                &delegation.target,
                crate::openai_live::OpenAiLiveDelegationTarget::Client
            ) =>
            {
                Some(ProviderConnectionEvent::DelegationRequested {
                    event_id,
                    delegation_id: delegation.id.0,
                    offset_ms: delegation.offset_ms,
                })
            }
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
    fn configuration_must_be_valid() {
        assert!(OpenAiLiveVoiceProvider::new("", OpenAiLiveVoiceConfig::default()).is_err());
        assert!(OpenAiLiveVoiceProvider::new(
            "key",
            OpenAiLiveVoiceConfig {
                voice: String::new(),
            },
        )
        .is_err());
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
                    start_ms: 10,
                    end_ms: 20,
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
