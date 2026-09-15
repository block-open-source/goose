//! OpenAI GPT-Live API.
//!
//! The client owns OpenAI configuration and protocol semantics. WebSocket and
//! WebRTC connectors own their distinct connection establishment flows.

use crate::{
    http_status::handle_response,
    live::{
        LiveProtocol, LiveSession, LiveSessionEvent, LiveTransport, LIVE_EVENT_CHANNEL_CAPACITY,
    },
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rmcp::model::Role;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use tokio::{
    sync::{broadcast, broadcast::error::RecvError},
    time::{timeout, Duration},
};
use uuid::Uuid;

pub const DEFAULT_OPENAI_LIVE_HTTP_URL: &str = "https://api.openai.com/v1/live/sessions";
pub const DEFAULT_OPENAI_LIVE_WEBSOCKET_URL: &str = "wss://api.openai.com/v1/live/sessions";
const SESSION_START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiLiveSessionConfig {
    pub model: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default)]
    pub input_messages: Vec<OpenAiLiveMessage>,
    #[serde(default)]
    pub extra_session_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiLiveMessage {
    pub role: OpenAiLiveMessageRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiLiveMessageRole {
    User,
    Assistant,
    Developer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiLiveContextChannel {
    Instructions,
    Thinking,
    Commentary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiLiveContext {
    pub text: String,
    pub channel: OpenAiLiveContextChannel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpenAiLiveSessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpenAiLiveDelegationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenAiLiveDelegationTarget {
    Client,
    Responses,
    Other(String),
}

impl From<String> for OpenAiLiveDelegationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for OpenAiLiveDelegationId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiLiveDelegation {
    pub id: OpenAiLiveDelegationId,
    pub target: OpenAiLiveDelegationTarget,
    pub offset_ms: u64,
}

pub enum OpenAiLiveCommand {
    AppendContext {
        event_id: String,
        delegation_id: Option<OpenAiLiveDelegationId>,
        context: OpenAiLiveContext,
    },
    AppendAudio(Vec<u8>),
    MuteInput {
        event_id: String,
    },
    UnmuteInput {
        event_id: String,
    },
    Close,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiLiveEvent {
    pub kind: OpenAiLiveEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenAiLiveEventKind {
    SessionStarted {
        event_id: String,
        session_id: OpenAiLiveSessionId,
    },
    TranscriptDelta {
        event_id: String,
        client_event_id: Option<String>,
        role: Role,
        delta: String,
        start_ms: u64,
        end_ms: u64,
    },
    OutputAudioDelta {
        audio: Vec<u8>,
        start_ms: Option<u64>,
        end_ms: Option<u64>,
    },
    DelegationCreated {
        event_id: String,
        client_event_id: Option<String>,
        delegation: OpenAiLiveDelegation,
    },
    ContextAppended {
        event_id: String,
        channel: OpenAiLiveContextChannel,
        client_event_id: Option<String>,
        start_ms: u64,
        end_ms: u64,
    },
    Usage {
        event_id: String,
        usage: Value,
    },
    Error {
        event_id: String,
        error_type: String,
        code: String,
        message: String,
        parameter: Option<String>,
        client_event_id: Option<String>,
    },
    Info {
        event_id: String,
        code: String,
        message: String,
        client_event_id: Option<String>,
    },
    InputMuted {
        event_id: String,
        client_event_id: Option<String>,
    },
    InputUnmuted {
        event_id: String,
        client_event_id: Option<String>,
    },
    SessionClosed {
        event_id: String,
        client_event_id: Option<String>,
        reason: String,
        session: Value,
        usage: Value,
    },
    Other {
        event_type: String,
    },
}

#[derive(Clone)]
pub struct OpenAiLiveClient {
    api_key: String,
    http_endpoint: String,
    websocket_endpoint: String,
}

impl OpenAiLiveClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http_endpoint: DEFAULT_OPENAI_LIVE_HTTP_URL.into(),
            websocket_endpoint: DEFAULT_OPENAI_LIVE_WEBSOCKET_URL.into(),
        }
    }

    pub fn from_env() -> Result<Self> {
        let key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is not set")?;
        if key.trim().is_empty() {
            bail!("OPENAI_API_KEY is empty");
        }
        Ok(Self::new(key))
    }

    pub fn with_http_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.http_endpoint = endpoint.into();
        self
    }

    pub fn with_websocket_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.websocket_endpoint = endpoint.into();
        self
    }

    pub fn websocket(&self, config: OpenAiLiveSessionConfig) -> OpenAiLiveWebSocketConnector {
        OpenAiLiveWebSocketConnector {
            client: self.clone(),
            config,
        }
    }

    pub fn existing_session(
        &self,
        session_id: OpenAiLiveSessionId,
    ) -> OpenAiLiveExistingSessionConnector {
        OpenAiLiveExistingSessionConnector {
            client: self.clone(),
            session_id,
        }
    }

    pub fn webrtc(&self, config: OpenAiLiveSessionConfig) -> OpenAiLiveWebRtcConnector {
        OpenAiLiveWebRtcConnector {
            client: self.clone(),
            config,
        }
    }

    fn connect_transport(
        &self,
        transport: Arc<dyn LiveTransport>,
        connection_kind: OpenAiLiveConnectionKind,
    ) -> StartingOpenAiLiveSession {
        let (session, events) = LiveSession::connect(Arc::new(OpenAiLiveProtocol), transport);
        StartingOpenAiLiveSession {
            session,
            events,
            pending_events: Default::default(),
            connection_kind,
        }
    }

    fn headers(&self) -> Vec<(String, String)> {
        vec![("Authorization".into(), format!("Bearer {}", self.api_key))]
    }

    fn session_json(config: &OpenAiLiveSessionConfig, websocket_audio: bool) -> Result<Value> {
        const RESERVED: &[&str] = &["model", "instructions", "input", "audio", "delegation"];
        if let Some(field) = config
            .extra_session_fields
            .keys()
            .find(|field| RESERVED.contains(&field.as_str()))
        {
            bail!("extra session field `{field}` conflicts with typed configuration");
        }
        let input_messages = config.input_messages.iter().map(|message| {
            let role = match message.role {
                OpenAiLiveMessageRole::User => "user",
                OpenAiLiveMessageRole::Assistant => "assistant",
                OpenAiLiveMessageRole::Developer => "developer",
            };
            let content_type = if matches!(message.role, OpenAiLiveMessageRole::Assistant) { "output_text" } else { "input_text" };
            json!({ "type": "message", "role": role, "content": [{ "type": content_type, "text": message.text }] })
        }).collect::<Vec<_>>();
        let mut session = json!({
            "model": config.model,
            "instructions": config.instructions,
            "input": input_messages,
            "delegation": { "type": "client" },
        });
        if websocket_audio || config.voice.is_some() {
            let mut audio = json!({});
            if websocket_audio {
                audio["format"] = json!({ "type": "audio/pcm", "rate": 24_000 });
            }
            if let Some(voice) = &config.voice {
                audio["output"] = json!({ "voice": voice });
            }
            session["audio"] = audio;
        }
        session
            .as_object_mut()
            .unwrap()
            .extend(config.extra_session_fields.clone());
        Ok(session)
    }
}

pub struct OpenAiLiveExistingSessionConnector {
    client: OpenAiLiveClient,
    session_id: OpenAiLiveSessionId,
}

impl OpenAiLiveExistingSessionConnector {
    pub fn request(self) -> Result<OpenAiLiveWebSocketRequest> {
        if self.session_id.0.trim().is_empty() {
            bail!("OpenAI Live session ID is empty");
        }
        Ok(OpenAiLiveWebSocketRequest {
            endpoint: format!(
                "{}/{}/attach",
                self.client.websocket_endpoint.trim_end_matches('/'),
                urlencoding::encode(&self.session_id.0)
            ),
            headers: self.client.headers(),
            initial_messages: Vec::new(),
        })
    }

    #[cfg(feature = "live-websocket")]
    pub async fn connect(self) -> Result<ConnectedOpenAiLiveSession> {
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let transport = Arc::new(
            crate::live_transport_websocket::WebSocketLiveTransport::connect(self.request()?)
                .await?,
        );
        Ok(client
            .connect_transport(transport, OpenAiLiveConnectionKind::MediaControl)
            .connected(session_id))
    }
}

pub struct OpenAiLiveWebSocketRequest {
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub initial_messages: Vec<Value>,
}

pub struct OpenAiLiveWebSocketConnector {
    client: OpenAiLiveClient,
    config: OpenAiLiveSessionConfig,
}

impl OpenAiLiveWebSocketConnector {
    pub fn request(self) -> Result<OpenAiLiveWebSocketRequest> {
        Ok(OpenAiLiveWebSocketRequest {
            endpoint: self.client.websocket_endpoint.clone(),
            headers: self.client.headers(),
            initial_messages: vec![json!({
                "type": "session.start",
                "event_id": event_id(),
                "session": OpenAiLiveClient::session_json(&self.config, true)?,
            })],
        })
    }

    #[cfg(feature = "live-websocket")]
    pub async fn connect(self) -> Result<ConnectedOpenAiLiveSession> {
        let client = self.client.clone();
        let transport = Arc::new(
            crate::live_transport_websocket::WebSocketLiveTransport::connect(self.request()?)
                .await?,
        );
        client
            .connect_transport(transport, OpenAiLiveConnectionKind::PrimaryWebSocket)
            .ready(None)
            .await
    }
}

pub struct OpenAiLiveWebRtcConnector {
    client: OpenAiLiveClient,
    config: OpenAiLiveSessionConfig,
}

pub struct OpenAiLiveWebRtcRequest {
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub offer_sdp: String,
    pub session: Value,
}

pub struct OpenAiLiveWebRtcNegotiation {
    pub answer_sdp: String,
    pub session_id: OpenAiLiveSessionId,
    client: OpenAiLiveClient,
}

impl OpenAiLiveWebRtcNegotiation {
    pub async fn bind(
        self,
        transport: Arc<dyn LiveTransport>,
    ) -> Result<ConnectedOpenAiLiveSession> {
        self.client
            .connect_transport(transport, OpenAiLiveConnectionKind::MediaControl)
            .ready(Some(self.session_id))
            .await
    }
}

impl OpenAiLiveWebRtcConnector {
    pub fn request(self, offer_sdp: impl Into<String>) -> Result<OpenAiLiveWebRtcRequest> {
        let offer_sdp = offer_sdp.into();
        if offer_sdp.trim().is_empty() {
            bail!("WebRTC SDP offer is empty");
        }
        Ok(OpenAiLiveWebRtcRequest {
            endpoint: self.client.http_endpoint.clone(),
            headers: self.client.headers(),
            offer_sdp,
            session: OpenAiLiveClient::session_json(&self.config, false)?,
        })
    }

    pub async fn negotiate(
        self,
        offer_sdp: impl Into<String>,
    ) -> Result<OpenAiLiveWebRtcNegotiation> {
        let client = self.client.clone();
        let request = self.request(offer_sdp)?;
        let mut http_request = reqwest::Client::new().post(request.endpoint).json(&json!({
            "session": request.session,
            "transport": { "type": "webrtc", "sdp": request.offer_sdp },
        }));
        for (name, value) in request.headers {
            http_request = http_request.header(name, value);
        }
        let result: Value = timeout(SESSION_START_TIMEOUT, async {
            let response = http_request.send().await?;
            handle_response(response).await
        })
        .await
        .context("OpenAI Live signaling timed out")??;
        let session_id = OpenAiLiveSessionId(required_nonempty_string(
            result.pointer("/session/id"),
            "session.id",
        )?);
        let answer_sdp =
            required_nonempty_string(result.pointer("/transport/sdp"), "transport.sdp")?;
        Ok(OpenAiLiveWebRtcNegotiation {
            answer_sdp,
            session_id,
            client,
        })
    }
}

pub type OpenAiLiveSession = LiveSession<OpenAiLiveProtocol>;
pub type OpenAiLiveSessionEvent = LiveSessionEvent<OpenAiLiveEvent>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAiLiveConnectionKind {
    PrimaryWebSocket,
    MediaControl,
}

struct StartingOpenAiLiveSession {
    session: OpenAiLiveSession,
    events: broadcast::Receiver<OpenAiLiveSessionEvent>,
    pending_events: std::collections::VecDeque<OpenAiLiveSessionEvent>,
    connection_kind: OpenAiLiveConnectionKind,
}

impl StartingOpenAiLiveSession {
    async fn ready(
        mut self,
        expected_session_id: Option<OpenAiLiveSessionId>,
    ) -> Result<ConnectedOpenAiLiveSession> {
        timeout(SESSION_START_TIMEOUT, async {
            loop {
                match self.events.recv().await {
                    Ok(LiveSessionEvent::Message(OpenAiLiveEvent {
                        kind: OpenAiLiveEventKind::SessionStarted { session_id, .. },
                        ..
                    })) => {
                        if expected_session_id
                            .as_ref()
                            .is_some_and(|expected| expected != &session_id)
                        {
                            bail!("OpenAI Live startup identity does not match creation identity");
                        }
                        return Ok(self.connected(session_id));
                    }
                    Ok(LiveSessionEvent::Message(event))
                        if matches!(event.kind, OpenAiLiveEventKind::Error { .. }) =>
                    {
                        if let OpenAiLiveEventKind::Error { message, .. } = event.kind {
                            bail!("OpenAI Live startup failed: {message}");
                        }
                    }
                    Ok(LiveSessionEvent::Ended { error, .. }) => {
                        if let Some(error) = error {
                            return Err(anyhow::anyhow!(error.to_string()));
                        }
                        bail!("OpenAI Live session ended before startup");
                    }
                    Ok(event @ LiveSessionEvent::Message(_)) => {
                        if self.pending_events.len() >= LIVE_EVENT_CHANNEL_CAPACITY {
                            bail!("too many events received before OpenAI Live session startup");
                        }
                        self.pending_events.push_back(event);
                    }
                    Err(RecvError::Lagged(count)) => {
                        bail!("OpenAI Live authoritative event receiver lagged by {count} events")
                    }
                    Err(RecvError::Closed) => {
                        bail!("OpenAI Live authoritative event stream closed")
                    }
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("OpenAI Live session startup timed out"))?
    }

    fn connected(self, session_id: OpenAiLiveSessionId) -> ConnectedOpenAiLiveSession {
        ConnectedOpenAiLiveSession {
            session: self.session,
            events: self.events,
            pending_events: self.pending_events,
            connection_kind: self.connection_kind,
            session_id,
        }
    }
}

pub struct ConnectedOpenAiLiveSession {
    session: OpenAiLiveSession,
    events: broadcast::Receiver<OpenAiLiveSessionEvent>,
    pending_events: std::collections::VecDeque<OpenAiLiveSessionEvent>,
    connection_kind: OpenAiLiveConnectionKind,
    session_id: OpenAiLiveSessionId,
}

impl ConnectedOpenAiLiveSession {
    pub async fn recv(&mut self) -> std::result::Result<OpenAiLiveSessionEvent, RecvError> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(event);
        }
        self.events.recv().await
    }

    pub fn session_id(&self) -> &OpenAiLiveSessionId {
        &self.session_id
    }

    pub async fn send(&self, command: OpenAiLiveCommand) -> Result<()> {
        if self.connection_kind != OpenAiLiveConnectionKind::PrimaryWebSocket
            && matches!(&command, OpenAiLiveCommand::AppendAudio(_))
        {
            bail!("audio input is only permitted on an OpenAI Live primary WebSocket");
        }
        self.session.send(command).await
    }

    pub async fn close(&self) -> Result<()> {
        self.session.close().await
    }
}

pub struct OpenAiLiveProtocol;

impl LiveProtocol for OpenAiLiveProtocol {
    type Command = OpenAiLiveCommand;
    type Event = OpenAiLiveEvent;

    fn encode(&self, command: Self::Command) -> Result<Value> {
        match command {
            OpenAiLiveCommand::AppendContext {
                event_id,
                delegation_id,
                context,
            } => text_event(event_id, delegation_id, context),
            OpenAiLiveCommand::AppendAudio(audio) => {
                if audio.is_empty() {
                    bail!("audio payload is empty");
                }
                Ok(json!({
                    "type": "session.input_audio.append",
                    "event_id": event_id(),
                    "audio": BASE64.encode(audio),
                }))
            }
            OpenAiLiveCommand::MuteInput { event_id } => {
                Ok(json!({ "type": "session.input_audio.mute", "event_id": event_id }))
            }
            OpenAiLiveCommand::UnmuteInput { event_id } => {
                Ok(json!({ "type": "session.input_audio.unmute", "event_id": event_id }))
            }
            OpenAiLiveCommand::Close => {
                Ok(json!({ "type": "session.close", "event_id": event_id() }))
            }
        }
    }

    fn decode(&self, event: Value) -> Result<Self::Event> {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .context("OpenAI Live event is missing type")?;
        let kind = match event_type {
            "session.started" => OpenAiLiveEventKind::SessionStarted {
                event_id: required_string(event.get("event_id"), "event_id")?,
                session_id: OpenAiLiveSessionId(required_nonempty_string(
                    event.pointer("/session/id"),
                    "session.id",
                )?),
            },
            "session.input_transcript.delta" => transcript_delta(&event, Role::User)?,
            "session.output_transcript.delta" => transcript_delta(&event, Role::Assistant)?,
            "session.output_audio.delta" => OpenAiLiveEventKind::OutputAudioDelta {
                audio: BASE64.decode(
                    event
                        .get("delta")
                        .and_then(Value::as_str)
                        .context("session.output_audio.delta is missing delta")?,
                )?,
                start_ms: optional_u64(event.get("start_ms"), "start_ms")?,
                end_ms: optional_u64(event.get("end_ms"), "end_ms")?,
            },
            "session.delegation.created" => {
                let delegation = event
                    .get("delegation")
                    .context("session.delegation.created is missing delegation")?;
                OpenAiLiveEventKind::DelegationCreated {
                    event_id: required_string(event.get("event_id"), "event_id")?,
                    client_event_id: optional_string(
                        event.get("client_event_id"),
                        "client_event_id",
                    )?,
                    delegation: OpenAiLiveDelegation {
                        id: delegation
                            .get("id")
                            .and_then(Value::as_str)
                            .context("delegation is missing id")?
                            .to_owned()
                            .into(),
                        target: delegation_target(delegation)?,
                        offset_ms: required_u64(event.get("offset_ms"), "offset_ms")?,
                    },
                }
            }
            "session.instructions.appended" => {
                context_appended(&event, OpenAiLiveContextChannel::Instructions)?
            }
            "session.thinking.appended" => {
                context_appended(&event, OpenAiLiveContextChannel::Thinking)?
            }
            "session.commentary.appended" => {
                context_appended(&event, OpenAiLiveContextChannel::Commentary)?
            }
            "session.usage.updated" => OpenAiLiveEventKind::Usage {
                event_id: required_string(event.get("event_id"), "event_id")?,
                usage: required_object(event.get("usage"), "usage")?,
            },
            "session.input_audio.muted" => OpenAiLiveEventKind::InputMuted {
                event_id: required_string(event.get("event_id"), "event_id")?,
                client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
            },
            "session.input_audio.unmuted" => OpenAiLiveEventKind::InputUnmuted {
                event_id: required_string(event.get("event_id"), "event_id")?,
                client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
            },
            "session.closed" => OpenAiLiveEventKind::SessionClosed {
                event_id: required_string(event.get("event_id"), "event_id")?,
                client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
                reason: required_string(event.get("reason"), "reason")?,
                session: required_object(event.get("session"), "session")?,
                usage: required_object(event.get("usage"), "usage")?,
            },
            "error" => live_error(&event, "/error")?,
            "info" => OpenAiLiveEventKind::Info {
                event_id: required_string(event.get("event_id"), "event_id")?,
                code: required_string(event.get("code"), "code")?,
                message: required_string(event.get("message"), "message")?,
                client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
            },
            _ => OpenAiLiveEventKind::Other {
                event_type: event_type.into(),
            },
        };
        Ok(OpenAiLiveEvent {
            kind,
            raw: Some(event),
        })
    }

    fn close_command(&self) -> Option<Self::Command> {
        Some(OpenAiLiveCommand::Close)
    }

    fn is_close_acknowledgement(&self, event: &Self::Event) -> bool {
        matches!(event.kind, OpenAiLiveEventKind::SessionClosed { .. })
    }
}

fn transcript_delta(event: &Value, role: Role) -> Result<OpenAiLiveEventKind> {
    Ok(OpenAiLiveEventKind::TranscriptDelta {
        event_id: required_string(event.get("event_id"), "event_id")?,
        client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
        role,
        delta: required_string(event.get("delta"), "delta")?,
        start_ms: required_u64(event.get("start_ms"), "start_ms")?,
        end_ms: required_u64(event.get("end_ms"), "end_ms")?,
    })
}

fn context_appended(
    event: &Value,
    channel: OpenAiLiveContextChannel,
) -> Result<OpenAiLiveEventKind> {
    Ok(OpenAiLiveEventKind::ContextAppended {
        event_id: required_string(event.get("event_id"), "event_id")?,
        channel,
        client_event_id: optional_string(event.get("client_event_id"), "client_event_id")?,
        start_ms: required_u64(event.get("start_ms"), "start_ms")?,
        end_ms: required_u64(event.get("end_ms"), "end_ms")?,
    })
}

fn delegation_target(item: &Value) -> Result<OpenAiLiveDelegationTarget> {
    match item
        .get("target")
        .and_then(Value::as_str)
        .context("delegation is missing target")?
    {
        "client" => Ok(OpenAiLiveDelegationTarget::Client),
        "responses" => Ok(OpenAiLiveDelegationTarget::Responses),
        target => Ok(OpenAiLiveDelegationTarget::Other(target.to_owned())),
    }
}

fn required_object(value: Option<&Value>, field: &str) -> Result<Value> {
    match value {
        Some(value) if value.is_object() => Ok(value.clone()),
        Some(_) => bail!("OpenAI Live event field {field} is not an object"),
        None => bail!("OpenAI Live event is missing {field}"),
    }
}

fn live_error(event: &Value, pointer: &str) -> Result<OpenAiLiveEventKind> {
    let error = event
        .pointer(pointer)
        .with_context(|| format!("OpenAI Live error is missing {pointer}"))?;
    Ok(OpenAiLiveEventKind::Error {
        event_id: required_string(event.get("event_id"), "event_id")?,
        error_type: required_string(error.get("type"), "error.type")?,
        code: required_string(error.get("code"), "error.code")?,
        message: required_string(error.get("message"), "error.message")?,
        parameter: optional_string(error.get("param"), "error.param")?,
        client_event_id: optional_string(error.get("client_event_id"), "error.client_event_id")?,
    })
}

fn required_string(value: Option<&Value>, field: &str) -> Result<String> {
    value
        .and_then(Value::as_str)
        .with_context(|| format!("OpenAI Live event is missing {field}"))
        .map(str::to_owned)
}

fn required_nonempty_string(value: Option<&Value>, field: &str) -> Result<String> {
    let value = required_string(value, field)?;
    if value.trim().is_empty() {
        bail!("OpenAI Live field {field} is empty");
    }
    Ok(value)
}

fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.to_owned())),
        Some(_) => bail!("OpenAI Live event field {field} is not a string"),
    }
}

fn required_u64(value: Option<&Value>, field: &str) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .with_context(|| format!("OpenAI Live event is missing {field}"))
}

fn optional_u64(value: Option<&Value>, field: &str) -> Result<Option<u64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .with_context(|| format!("OpenAI Live event field {field} is not an unsigned integer"))
            .map(Some),
    }
}

fn event_id() -> String {
    format!("event_{}", Uuid::new_v4())
}

fn text_event(
    event_id: String,
    delegation_id: Option<OpenAiLiveDelegationId>,
    context: OpenAiLiveContext,
) -> Result<Value> {
    let kind = match context.channel {
        OpenAiLiveContextChannel::Instructions => "session.instructions.append",
        OpenAiLiveContextChannel::Thinking => "session.thinking.append",
        OpenAiLiveContextChannel::Commentary => "session.commentary.append",
    };
    if event_id.trim().is_empty() {
        bail!("event ID is empty");
    }
    if delegation_id
        .as_ref()
        .is_some_and(|delegation_id| delegation_id.0.trim().is_empty())
    {
        bail!("delegation ID is empty");
    }
    Ok(json!({
        "type": kind,
        "event_id": event_id,
        "delegation_id": delegation_id.map(|delegation_id| delegation_id.0),
        "content": context.text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_live_transport::BrowserLiveTransport;
    use wiremock::{
        matchers::{body_json, header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn config() -> OpenAiLiveSessionConfig {
        OpenAiLiveSessionConfig {
            model: "gpt-live-1".into(),
            instructions: "help".into(),
            voice: None,
            input_messages: vec![],
            extra_session_fields: Default::default(),
        }
    }

    fn decode(event: Value) -> OpenAiLiveEventKind {
        OpenAiLiveProtocol.decode(event).unwrap().kind
    }

    #[test]
    fn existing_session_sideband_is_authenticated_without_primary_bootstrap() {
        let request = OpenAiLiveClient::new("test-key")
            .existing_session(OpenAiLiveSessionId("session/a".into()))
            .request()
            .unwrap();
        assert!(request.endpoint.ends_with("/session%2Fa/attach"));
        assert!(request
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Bearer test-key"));
        assert!(request.initial_messages.is_empty());
    }

    #[test]
    fn primary_websocket_bootstraps_with_session_start_and_pcm_audio() {
        let request = OpenAiLiveClient::new("test")
            .websocket(config())
            .request()
            .unwrap();
        assert_eq!(request.endpoint, DEFAULT_OPENAI_LIVE_WEBSOCKET_URL);
        assert_eq!(request.initial_messages[0]["type"], "session.start");
        assert_eq!(
            request.initial_messages[0]["session"]["audio"]["format"],
            json!({ "type": "audio/pcm", "rate": 24_000 })
        );
    }

    #[test]
    fn initial_message_roles_use_valid_content_types() {
        let roles = [
            (OpenAiLiveMessageRole::Developer, "developer", "input_text"),
            (OpenAiLiveMessageRole::User, "user", "input_text"),
            (OpenAiLiveMessageRole::Assistant, "assistant", "output_text"),
        ];
        for (role, expected_role, expected_content_type) in roles {
            let mut config = config();
            config.input_messages.push(OpenAiLiveMessage {
                role,
                text: "hello".into(),
            });
            let session = OpenAiLiveClient::session_json(&config, false).unwrap();
            assert_eq!(session["input"][0]["role"], expected_role);
            assert_eq!(
                session["input"][0]["content"][0]["type"],
                expected_content_type
            );
        }
    }

    #[tokio::test]
    async fn webrtc_negotiation_uses_ga_request_and_response_shapes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/live/sessions"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_json(json!({
                "session": {
                    "model": "gpt-live-1",
                    "instructions": "help",
                    "input": [],
                    "delegation": { "type": "client" }
                },
                "transport": { "type": "webrtc", "sdp": "offer-sdp" }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "session": { "id": "session_1" },
                "transport": { "type": "webrtc", "sdp": "answer-sdp" }
            })))
            .mount(&server)
            .await;

        let negotiation = OpenAiLiveClient::new("test-key")
            .with_http_endpoint(format!("{}/v1/live/sessions", server.uri()))
            .webrtc(config())
            .negotiate("offer-sdp")
            .await
            .unwrap();

        assert_eq!(negotiation.session_id.0, "session_1");
        assert_eq!(negotiation.answer_sdp, "answer-sdp");
    }

    #[test]
    fn session_identity_must_not_be_empty() {
        let error = OpenAiLiveProtocol
            .decode(json!({
                "type": "session.started",
                "event_id": "event_started_1",
                "session": { "id": "  " }
            }))
            .unwrap_err();

        assert!(error.to_string().contains("session.id is empty"));
    }

    #[tokio::test]
    async fn ready_buffers_events_before_session_started() {
        let transport = Arc::new(BrowserLiveTransport::new());
        let connected = OpenAiLiveClient::new("test").connect_transport(
            transport.clone(),
            OpenAiLiveConnectionKind::PrimaryWebSocket,
        );
        transport
            .push_incoming(json!({
                "type": "session.usage.updated",
                "event_id": "event_usage_1",
                "usage": {}
            }))
            .await
            .unwrap();
        transport
            .push_incoming(json!({
                "type": "session.started",
                "event_id": "event_started_1",
                "session": { "id": "session_1" }
            }))
            .await
            .unwrap();

        let mut connected = connected.ready(None).await.unwrap();
        assert_eq!(connected.session_id().0, "session_1");
        assert!(matches!(
            connected.recv().await.unwrap(),
            LiveSessionEvent::Message(OpenAiLiveEvent {
                kind: OpenAiLiveEventKind::Usage { .. },
                ..
            })
        ));
    }

    #[tokio::test]
    async fn ready_rejects_a_different_creation_identity() {
        let transport = Arc::new(BrowserLiveTransport::new());
        let starting = OpenAiLiveClient::new("test")
            .connect_transport(transport.clone(), OpenAiLiveConnectionKind::MediaControl);
        transport
            .push_incoming(json!({
                "type": "session.started",
                "event_id": "event_started_1",
                "session": { "id": "session_2" }
            }))
            .await
            .unwrap();

        let Err(error) = starting
            .ready(Some(OpenAiLiveSessionId("session_1".into())))
            .await
        else {
            panic!("expected a mismatched session identity");
        };

        assert!(error.to_string().contains("creation identity"));
    }

    #[tokio::test]
    async fn authoritative_receiver_reports_lag() {
        let transport = Arc::new(BrowserLiveTransport::new());
        let mut connected = OpenAiLiveClient::new("test")
            .connect_transport(
                transport.clone(),
                OpenAiLiveConnectionKind::PrimaryWebSocket,
            )
            .connected(OpenAiLiveSessionId("session_1".into()));

        for sequence in 0..600 {
            transport
                .push_incoming(json!({ "type": "future.event", "sequence": sequence }))
                .await
                .unwrap();
        }

        assert!(matches!(connected.recv().await, Err(RecvError::Lagged(_))));
    }

    #[test]
    fn transcript_deltas_keep_role_and_timing() {
        let output = json!({
            "type": "session.output_transcript.delta",
            "event_id": "event_output_1",
            "delta": " It's five",
            "start_ms": 3200,
            "end_ms": 3400
        });
        let OpenAiLiveEventKind::TranscriptDelta {
            role,
            delta,
            start_ms,
            end_ms,
            ..
        } = decode(output)
        else {
            panic!("expected transcript delta");
        };
        assert_eq!(role, Role::Assistant);
        assert_eq!(delta, " It's five");
        assert_eq!((start_ms, end_ms), (3200, 3400));

        let input = json!({
            "type": "session.input_transcript.delta",
            "event_id": "event_input_1",
            "delta": "What",
            "start_ms": 400,
            "end_ms": 600
        });
        assert!(
            matches!(decode(input), OpenAiLiveEventKind::TranscriptDelta { role, .. }
            if role == Role::User)
        );
    }

    #[test]
    fn delegation_keeps_identity_target_and_offset() {
        let delegation = json!({
            "type": "session.delegation.created",
            "event_id": "event_delegation_1",
            "offset_ms": 800,
            "delegation": { "id": "delegation_1", "type": "delegation", "target": "client" }
        });
        let OpenAiLiveEventKind::DelegationCreated { delegation, .. } = decode(delegation) else {
            panic!("expected delegation");
        };
        assert_eq!(delegation.id.0, "delegation_1");
        assert_eq!(delegation.target, OpenAiLiveDelegationTarget::Client);
        assert_eq!(delegation.offset_ms, 800);

        let unsupported = json!({
            "type": "session.delegation.created",
            "event_id": "event_delegation_2",
            "offset_ms": 900,
            "delegation": { "id": "delegation_2", "target": "future_target" }
        });
        let OpenAiLiveEventKind::DelegationCreated { delegation, .. } = decode(unsupported) else {
            panic!("expected delegation");
        };
        assert!(
            matches!(delegation.target, OpenAiLiveDelegationTarget::Other(target)
            if target == "future_target")
        );
    }

    #[test]
    fn context_acknowledgements_and_errors_preserve_correlation() {
        let accepted = json!({ "type": "session.commentary.appended", "event_id": "event_accepted_1",
            "client_event_id": "client_event_1", "start_ms": 800, "end_ms": 1200 });
        let rejected = json!({ "type": "error", "event_id": "event_error_1", "error": { "type": "invalid_request_error",
            "code": "empty_array", "message": "content cannot be empty", "param": "content",
            "client_event_id": "client_event_2" } });

        let OpenAiLiveEventKind::ContextAppended {
            channel,
            client_event_id,
            start_ms,
            end_ms,
            ..
        } = decode(accepted)
        else {
            panic!("expected context acknowledgement");
        };
        assert!(matches!(channel, OpenAiLiveContextChannel::Commentary));
        assert_eq!(client_event_id.as_deref(), Some("client_event_1"));
        assert_eq!(start_ms, 800);
        assert_eq!(end_ms, 1200);

        let OpenAiLiveEventKind::Error {
            error_type,
            code,
            message,
            parameter,
            client_event_id,
            ..
        } = decode(rejected)
        else {
            panic!("expected error");
        };
        assert_eq!(error_type, "invalid_request_error");
        assert_eq!(code, "empty_array");
        assert_eq!(message, "content cannot be empty");
        assert_eq!(parameter.as_deref(), Some("content"));
        assert_eq!(client_event_id.as_deref(), Some("client_event_2"));
    }

    #[test]
    fn close_mute_and_unknown_events_preserve_semantics() {
        let closed = json!({ "type": "session.closed", "event_id": "event_closed_1",
            "client_event_id": "client_close_1", "reason": "close_requested",
            "session": { "id": "session_1" }, "usage": { "seconds": 8 } });

        let OpenAiLiveEventKind::SessionClosed { reason, usage, .. } = decode(closed) else {
            panic!("expected session close");
        };
        assert_eq!(reason, "close_requested");
        assert_eq!(usage["seconds"], 8);
        assert!(usage.get("total_tokens").is_none());
        assert!(matches!(
            decode(json!({ "type": "session.input_audio.muted", "event_id": "event_muted_1" })),
            OpenAiLiveEventKind::InputMuted {
                client_event_id: None,
                ..
            }
        ));
        assert!(matches!(
            decode(json!({ "type": "session.input_audio.unmuted", "event_id": "event_unmuted_1" })),
            OpenAiLiveEventKind::InputUnmuted {
                client_event_id: None,
                ..
            }
        ));
        assert!(matches!(
            decode(json!({ "type": "future.event" })),
            OpenAiLiveEventKind::Other { event_type } if event_type == "future.event"
        ));
        assert_eq!(
            OpenAiLiveProtocol.encode(OpenAiLiveCommand::Close).unwrap()["type"],
            "session.close"
        );
    }

    #[test]
    fn malformed_supported_event_is_rejected() {
        let error = OpenAiLiveProtocol
            .decode(json!({
                "type": "session.input_transcript.delta",
                "event_id": "event_transcript_1",
                "start_ms": 0,
                "end_ms": 100
            }))
            .unwrap_err();
        assert!(error.to_string().contains("delta"));
    }

    #[test]
    fn output_audio_preserves_sideband_timing() {
        let event = json!({
            "type": "session.output_audio.delta",
            "delta": BASE64.encode([0_u8, 1, 2]),
            "start_ms": 100,
            "end_ms": 200
        });
        let OpenAiLiveEventKind::OutputAudioDelta {
            audio,
            start_ms,
            end_ms,
        } = decode(event)
        else {
            panic!("expected output audio");
        };
        assert_eq!(audio, vec![0, 1, 2]);
        assert_eq!(start_ms, Some(100));
        assert_eq!(end_ms, Some(200));
    }

    #[tokio::test]
    async fn media_control_connection_rejects_primary_audio_commands() {
        let transport = Arc::new(BrowserLiveTransport::new());
        let connected = OpenAiLiveClient::new("test")
            .connect_transport(transport, OpenAiLiveConnectionKind::MediaControl)
            .connected(OpenAiLiveSessionId("session_1".into()));

        let error = connected
            .send(OpenAiLiveCommand::AppendAudio(vec![1]))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("only permitted"));
    }

    #[test]
    fn encodes_context_with_optional_delegation() {
        let cases = [
            (
                OpenAiLiveContextChannel::Commentary,
                Some(OpenAiLiveDelegationId("delegation_1".into())),
                "session.commentary.append",
                json!("delegation_1"),
            ),
            (
                OpenAiLiveContextChannel::Instructions,
                None,
                "session.instructions.append",
                Value::Null,
            ),
        ];

        for (channel, delegation_id, expected_type, expected_delegation_id) in cases {
            let encoded = OpenAiLiveProtocol
                .encode(OpenAiLiveCommand::AppendContext {
                    event_id: "client_event_1".into(),
                    delegation_id,
                    context: OpenAiLiveContext {
                        text: "context".into(),
                        channel,
                    },
                })
                .unwrap();
            assert_eq!(encoded["type"], expected_type);
            assert_eq!(encoded["delegation_id"], expected_delegation_id);
            assert_eq!(encoded["event_id"], "client_event_1");
            assert_eq!(encoded["content"], "context");
        }
    }
}
