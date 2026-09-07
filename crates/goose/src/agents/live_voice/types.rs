use goose_providers::live_voice_provider::{
    LiveVoiceConfig, LiveVoiceMediaAnswer, LiveVoiceMediaRequest, ProjectedTurn,
    ProviderDelegationId, ProviderStartupObservation, ProviderUsage, TranscriptFragment,
};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LiveOwnerToken(pub String);
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LiveSessionId(pub String);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkSessionId(pub String);
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LiveConnectionId(pub String);
pub type StartAttempt = u64;

pub struct StartRequest {
    pub owner: LiveOwnerToken,
    pub live_session_id: LiveSessionId,
    pub linked_work_session_id: Option<WorkSessionId>,
    pub attempt: StartAttempt,
    pub config: LiveVoiceConfig,
    pub media: LiveVoiceMediaRequest,
    pub updates: mpsc::UnboundedSender<LiveVoiceUpdate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartResponse {
    pub live_session_id: LiveSessionId,
    pub linked_work_session_id: Option<WorkSessionId>,
    pub attempt: StartAttempt,
    pub live_connection_id: LiveConnectionId,
    pub media_answer: LiveVoiceMediaAnswer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaReadinessObservation {
    AnswerApplied,
    RemoteTrackLive,
    PeerConnected,
    DataChannelOpen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveVoiceCommand {
    ObserveMedia(MediaReadinessObservation),
    ObserveBootstrap(ProviderStartupObservation),
    SetMuted(bool),
    MediaFailed(String),
    MediaCleanupResult(MediaCleanupResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopTarget {
    Attempt(StartAttempt),
    Connection(LiveConnectionId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveConnectionState {
    Connecting,
    Active,
    Ending,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCleanupResult {
    Dispatched,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveTerminalReason {
    Stopped,
    Cancelled,
    OwnerLost,
    SessionClosed,
    ReadinessTimedOut,
    MediaFailed(String),
    ProviderClosed(Option<String>),
    ProviderFailed(String),
    EventContinuityLost(u64),
    CommandFailed(String),
    StartFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveVoiceEvent {
    State {
        connection: LiveConnectionState,
        muted_intent: bool,
        provider_input_enabled: bool,
        output_speaking: Option<bool>,
    },
    Transcript(TranscriptFragment),
    ProjectedTurn(ProjectedTurn),
    DelegationUnsupportedTarget(ProviderDelegationId),
    DelegationPending(ProviderDelegationId),
    DelegationDelivered(ProviderDelegationId),
    DelegationFailed(ProviderDelegationId, String),
    RequestMediaCleanup(Duration),
    Terminal {
        reason: LiveTerminalReason,
        remote_acknowledged: bool,
        provider_close_completed: bool,
        media_cleanup: Option<MediaCleanupResult>,
        usage: Option<ProviderUsage>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveVoiceUpdate {
    pub live_session_id: LiveSessionId,
    pub linked_work_session_id: Option<WorkSessionId>,
    pub attempt: StartAttempt,
    pub live_connection_id: LiveConnectionId,
    pub event: LiveVoiceEvent,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoordinatorError {
    #[error("start attempt must exceed the highest attempt seen")]
    StaleAttempt,
    #[error("this Live session already has a connection for this owner")]
    Busy,
    #[error("the Live connection is not current")]
    StaleConnection,
    #[error("Live provider setup failed: {0}")]
    StartFailed(String),
    #[error("Live start was cancelled")]
    Cancelled,
}
