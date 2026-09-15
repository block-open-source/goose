use super::call::{LiveCallRuntime, LiveMainAgent, LiveVoiceCall, LiveVoiceCallId};
use crate::acp::server::ActiveRunRegistry;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageContent};
use crate::conversation::Conversation;
use crate::session::SessionManager;
use crate::token_counter::TokenCounter;
use goose_providers::live_voice_provider::{LiveVoiceInputMessage, LiveVoiceProvider};
pub(super) use goose_providers::live_voice_provider::{WebRtcAnswer, WebRtcOffer};
use goose_providers::openai_live_voice_provider::{OpenAiLiveVoiceConfig, OpenAiLiveVoiceProvider};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const LIVE_VOICE_INPUT_MESSAGE_LIMIT: usize = 128;
const LIVE_VOICE_INPUT_TOKEN_LIMIT: usize = 8_192;
const LIVE_VOICE_ENABLED_CONFIG_KEY: &str = "GOOSE_LIVE_VOICE_ENABLED";
const LIVE_VOICE_CONFIG_KEY: &str = "GOOSE_LIVE_VOICE";

pub(super) type LiveCallControls = Arc<Mutex<HashMap<String, LiveCallControl>>>;
pub(super) type LiveVoiceTranscriptPublisher = Arc<dyn Fn(Message) + Send + Sync>;
type LiveVoiceResolver =
    Arc<dyn Fn() -> Result<Arc<dyn LiveVoiceProvider>, &'static str> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveVoiceCallCompletion {
    Stopped,
    Failed,
}

pub(super) struct StartLiveVoiceCallResult {
    pub(super) call_id: LiveVoiceCallId,
    pub(super) answer: WebRtcAnswer,
    pub(super) completion_rx: watch::Receiver<Option<LiveVoiceCallCompletion>>,
}

#[derive(Debug)]
pub(super) enum LiveVoiceError {
    Unavailable,
    StartFailed,
    StopFailed,
}

/// Control side of the task that exclusively owns `LiveVoiceCall`.
pub(super) struct LiveCallControl {
    call_id: LiveVoiceCallId,
    stop_requested: CancellationToken,
    completion_rx: watch::Receiver<Option<LiveVoiceCallCompletion>>,
}

impl LiveCallControl {
    fn request_stop(&self) -> watch::Receiver<Option<LiveVoiceCallCompletion>> {
        self.stop_requested.cancel();
        self.completion_rx.clone()
    }
}

pub(super) struct LiveCallGuard {
    active_runs: Arc<ActiveRunRegistry>,
    session_id: String,
}

impl LiveCallGuard {
    fn start(active_runs: Arc<ActiveRunRegistry>, session_id: &str) -> Option<Self> {
        active_runs.start_live(session_id).then(|| Self {
            active_runs,
            session_id: session_id.to_string(),
        })
    }
}

impl Drop for LiveCallGuard {
    fn drop(&mut self) {
        self.active_runs.finish_live(&self.session_id);
    }
}

pub struct LiveVoiceService {
    live_voice_resolver: LiveVoiceResolver,
    calls_by_session: LiveCallControls,
    active_runs: Arc<ActiveRunRegistry>,
}

impl LiveVoiceService {
    pub fn from_config(active_runs: Arc<ActiveRunRegistry>) -> Self {
        Self::new(Arc::new(configured_live_voice), active_runs)
    }

    fn new(live_voice_resolver: LiveVoiceResolver, active_runs: Arc<ActiveRunRegistry>) -> Self {
        Self {
            live_voice_resolver,
            calls_by_session: Arc::new(Mutex::new(HashMap::new())),
            active_runs,
        }
    }

    pub(super) fn availability(
        &self,
        session_id: &str,
        mode: GooseMode,
    ) -> Result<(), &'static str> {
        self.eligible_provider(session_id, mode).map(|_| ())
    }

    fn eligible_provider(
        &self,
        session_id: &str,
        mode: GooseMode,
    ) -> Result<Arc<dyn LiveVoiceProvider>, &'static str> {
        let provider = (self.live_voice_resolver)()?;

        if mode != GooseMode::Auto {
            Err("Live voice requires Autonomous mode")
        } else if self.active_runs.is_active(session_id) {
            Err("Live voice is unavailable while this session is busy")
        } else {
            Ok(provider)
        }
    }

    pub(super) async fn start_call(
        &self,
        session_id: &str,
        offer: WebRtcOffer,
        session_manager: Arc<SessionManager>,
        transcript_publisher: LiveVoiceTranscriptPublisher,
        main_agent: LiveMainAgent,
    ) -> Result<StartLiveVoiceCallResult, LiveVoiceError> {
        let session = session_manager
            .get_session(session_id, true)
            .await
            .map_err(|_| LiveVoiceError::Unavailable)?;
        let provider = self
            .eligible_provider(session_id, session.goose_mode)
            .map_err(|_| LiveVoiceError::Unavailable)?;
        let call_guard = LiveCallGuard::start(self.active_runs.clone(), session_id)
            .ok_or(LiveVoiceError::Unavailable)?;
        let input_messages =
            live_voice_input_messages(&session.conversation.unwrap_or_default()).await?;
        let (answer, provider_connection) = provider
            .start(offer, input_messages)
            .await
            .map_err(|_| LiveVoiceError::StartFailed)?;

        let call_id = LiveVoiceCallId::new();
        let call = LiveVoiceCall::new(session_id.to_string(), call_id.clone(), provider_connection);
        let stop_requested = CancellationToken::new();
        let (completion_tx, completion_rx) = watch::channel(None);
        let mut calls = self
            .calls_by_session
            .lock()
            .expect("live voice lock poisoned");
        calls.insert(
            session_id.to_string(),
            LiveCallControl {
                call_id: call_id.clone(),
                stop_requested: stop_requested.clone(),
                completion_rx: completion_rx.clone(),
            },
        );
        drop(calls);
        let runtime = LiveCallRuntime::new(
            self.calls_by_session.clone(),
            stop_requested,
            completion_tx,
            session_manager,
            transcript_publisher,
            main_agent,
            call_guard,
        );
        tokio::spawn(call.run(runtime));
        Ok(StartLiveVoiceCallResult {
            call_id,
            answer,
            completion_rx,
        })
    }

    pub(super) async fn stop_call(
        &self,
        session_id: &str,
        call_id: &LiveVoiceCallId,
    ) -> Result<(), LiveVoiceError> {
        let completion_rx = {
            let calls = self
                .calls_by_session
                .lock()
                .expect("live voice lock poisoned");
            let control = calls.get(session_id).ok_or(LiveVoiceError::Unavailable)?;
            if &control.call_id != call_id {
                return Err(LiveVoiceError::Unavailable);
            }
            control.request_stop()
        };
        let completion = wait_for_completion(completion_rx).await?;
        match completion {
            LiveVoiceCallCompletion::Stopped => Ok(()),
            LiveVoiceCallCompletion::Failed => Err(LiveVoiceError::StopFailed),
        }
    }
}

fn configured_live_voice_enabled() -> bool {
    crate::config::Config::global()
        .get_param::<serde_json::Value>(LIVE_VOICE_ENABLED_CONFIG_KEY)
        .is_ok_and(|value| match value {
            serde_json::Value::Bool(enabled) => enabled,
            serde_json::Value::Number(enabled) => enabled.as_u64() == Some(1),
            serde_json::Value::String(enabled) => {
                enabled == "1" || enabled.eq_ignore_ascii_case("true")
            }
            _ => false,
        })
}

fn configured_live_voice() -> Result<Arc<dyn LiveVoiceProvider>, &'static str> {
    if !configured_live_voice_enabled() || !crate::agents::state_machine::enabled() {
        return Err("Live voice is disabled");
    }

    let config = crate::config::Config::global();
    let api_key = config
        .get_secret::<String>("OPENAI_API_KEY")
        .map_err(|_| "Live voice provider is not configured")?;
    let provider_config = config
        .get_param::<String>(LIVE_VOICE_CONFIG_KEY)
        .map(|voice| OpenAiLiveVoiceConfig { voice })
        .unwrap_or_default();
    OpenAiLiveVoiceProvider::new(api_key, provider_config)
        .map(|provider| Arc::new(provider) as Arc<dyn LiveVoiceProvider>)
        .map_err(|_| "Live voice provider is not configured")
}

async fn live_voice_input_messages(
    conversation: &Conversation,
) -> Result<Vec<LiveVoiceInputMessage>, LiveVoiceError> {
    let messages = conversation
        .messages()
        .iter()
        .filter(|message| message.is_user_visible())
        .filter_map(|message| {
            let text = message
                .user_visible_content()
                .content
                .iter()
                .filter_map(MessageContent::as_text)
                .collect::<String>();
            if text.trim().is_empty() {
                return None;
            }
            Some(LiveVoiceInputMessage {
                role: message.role.clone(),
                text,
            })
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Ok(messages);
    }
    let token_counter = TokenCounter::new()
        .await
        .map_err(|_| LiveVoiceError::StartFailed)?;
    let mut start = messages.len();
    let mut tokens = 0;
    for (index, message) in messages.iter().enumerate().rev() {
        if messages.len() - start == LIVE_VOICE_INPUT_MESSAGE_LIMIT {
            break;
        }
        let message_tokens = token_counter.count_tokens(&message.text);
        if message_tokens > LIVE_VOICE_INPUT_TOKEN_LIMIT - tokens {
            break;
        }
        tokens += message_tokens;
        start = index;
    }
    Ok(messages.into_iter().skip(start).collect())
}

pub(super) async fn wait_for_completion(
    mut completion_rx: watch::Receiver<Option<LiveVoiceCallCompletion>>,
) -> Result<LiveVoiceCallCompletion, LiveVoiceError> {
    loop {
        if let Some(completion) = *completion_rx.borrow() {
            return Ok(completion);
        }
        completion_rx
            .changed()
            .await
            .map_err(|_| LiveVoiceError::StopFailed)?;
    }
}

pub(super) fn remove_call_if_current(
    calls_by_session: &LiveCallControls,
    session_id: &str,
    call_id: &LiveVoiceCallId,
) {
    let mut calls = calls_by_session.lock().expect("live voice lock poisoned");
    if matches!(
        calls.get(session_id),
        Some(control) if &control.call_id == call_id
    ) {
        calls.remove(session_id);
    }
}

#[cfg(test)]
mod tests;
