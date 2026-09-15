use super::call::{DelegationDecision, LiveVoiceCall, LiveVoiceCallId, DELEGATION_INSTRUCTION};
use crate::acp::server::ActiveRunRegistry;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageContent};
use crate::conversation::Conversation;
use crate::session::SessionManager;
use crate::token_counter::TokenCounter;
use futures::future::BoxFuture;
use goose_providers::live_voice_provider::{
    LiveVoiceInputMessage, LiveVoiceProvider, LiveVoiceProviderAvailability,
    ProviderConnectionEvent,
};
pub(super) use goose_providers::live_voice_provider::{WebRtcAnswer, WebRtcOffer};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const PROVIDER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_VOICE_INPUT_MESSAGE_COUNT: usize = 10;
const DELEGATION_UPDATE_TOKEN_LIMIT: usize = 500;
const SAVED_RESULT_NOTICE: &str = "\n\nThe full result is saved in Goose.";

type LiveCallControls = Arc<Mutex<HashMap<String, LiveCallControl>>>;
pub(super) type LiveVoiceCallEndedHandler = Arc<dyn Fn(LiveVoiceCallEnded) + Send + Sync>;
pub(super) type LiveVoiceTranscriptHandler = Arc<dyn Fn(Message) + Send + Sync>;
pub(super) type LiveVoiceDelegationHandler =
    Arc<dyn Fn(String, LiveVoiceDelegationCommand) -> BoxFuture<'static, String> + Send + Sync>;

pub(super) enum LiveVoiceDelegationCommand {
    Start { input: String },
    Steer { input: String },
}

struct PendingDelegation {
    provider_delegation_id: String,
    future: BoxFuture<'static, String>,
}

struct BackgroundFinalization {
    delegation: BoxFuture<'static, String>,
    deferred_transcript: Vec<Message>,
    pending_context: Option<String>,
}

struct LiveCallExit {
    completion: LiveVoiceCallCompletion,
    background_finalization: Option<BackgroundFinalization>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveVoiceCallCompletion {
    Stopped,
    Failed,
}

pub(super) struct LiveVoiceCallEnded {
    pub(super) session_id: String,
    pub(super) call_id: LiveVoiceCallId,
    pub(super) completion: LiveVoiceCallCompletion,
}

pub(super) struct StartLiveVoiceCallResult {
    pub(super) call_id: LiveVoiceCallId,
    pub(super) answer: WebRtcAnswer,
    pub(super) completion_rx: watch::Receiver<Option<LiveVoiceCallCompletion>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveVoiceAvailability {
    Ready,
    FeatureDisabled,
    ProviderUnavailable,
    ChatBusy,
    RequiresAutonomousMode,
}

#[derive(Debug)]
pub(super) enum LiveVoiceError {
    Unavailable,
    StartFailed,
    StopFailed,
}

/// Control side of the task that exclusively owns `LiveVoiceCall`.
struct LiveCallControl {
    run_guard: LiveRunGuard,
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

struct LiveRunGuard {
    active_runs: Arc<ActiveRunRegistry>,
    session_id: String,
}

impl LiveRunGuard {
    fn start(active_runs: Arc<ActiveRunRegistry>, session_id: &str) -> Option<Self> {
        active_runs.start_live(session_id).then(|| Self {
            active_runs,
            session_id: session_id.to_string(),
        })
    }
}

impl Drop for LiveRunGuard {
    fn drop(&mut self) {
        self.active_runs.finish_live(&self.session_id);
    }
}

pub struct LiveVoiceService {
    provider: Arc<dyn LiveVoiceProvider>,
    calls_by_session: LiveCallControls,
    active_runs: Arc<ActiveRunRegistry>,
}

impl LiveVoiceService {
    pub fn new(provider: Arc<dyn LiveVoiceProvider>, active_runs: Arc<ActiveRunRegistry>) -> Self {
        Self {
            provider,
            calls_by_session: Arc::new(Mutex::new(HashMap::new())),
            active_runs,
        }
    }

    pub(super) fn availability(&self, session_id: &str, mode: GooseMode) -> LiveVoiceAvailability {
        use LiveVoiceAvailability::*;

        match self.provider.availability() {
            LiveVoiceProviderAvailability::Disabled => return FeatureDisabled,
            LiveVoiceProviderAvailability::Unavailable => return ProviderUnavailable,
            LiveVoiceProviderAvailability::Ready => {}
        }

        if self.active_runs.is_active(session_id) {
            ChatBusy
        } else if mode != GooseMode::Auto {
            RequiresAutonomousMode
        } else {
            Ready
        }
    }

    pub(super) async fn start_call(
        &self,
        session_id: &str,
        offer: WebRtcOffer,
        session_manager: Arc<SessionManager>,
        transcript_handler: LiveVoiceTranscriptHandler,
        call_ended_handler: LiveVoiceCallEndedHandler,
        delegation_handler: LiveVoiceDelegationHandler,
    ) -> Result<StartLiveVoiceCallResult, LiveVoiceError> {
        if self.provider.availability() != LiveVoiceProviderAvailability::Ready {
            return Err(LiveVoiceError::Unavailable);
        }
        let session = session_manager
            .get_session(session_id, true)
            .await
            .map_err(|_| LiveVoiceError::Unavailable)?;
        if session.goose_mode != GooseMode::Auto
            || session.provider_name.is_none()
            || session.model_config.is_none()
        {
            return Err(LiveVoiceError::Unavailable);
        }
        let run_guard = LiveRunGuard::start(self.active_runs.clone(), session_id)
            .ok_or(LiveVoiceError::Unavailable)?;
        let input_messages = live_voice_input_messages(&session.conversation.unwrap_or_default());
        let (answer, provider_connection) = self
            .provider
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
                run_guard,
                call_id: call_id.clone(),
                stop_requested: stop_requested.clone(),
                completion_rx: completion_rx.clone(),
            },
        );
        drop(calls);
        spawn_live_call(
            self.calls_by_session.clone(),
            call,
            stop_requested,
            completion_tx,
            session_manager,
            transcript_handler,
            call_ended_handler,
            delegation_handler,
        );
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

fn live_voice_input_messages(conversation: &Conversation) -> Vec<LiveVoiceInputMessage> {
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
    let start = messages
        .len()
        .saturating_sub(LIVE_VOICE_INPUT_MESSAGE_COUNT);
    messages.into_iter().skip(start).collect()
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

fn spawn_live_call(
    calls_by_session: LiveCallControls,
    mut call: LiveVoiceCall,
    stop_requested: CancellationToken,
    completion_tx: watch::Sender<Option<LiveVoiceCallCompletion>>,
    session_manager: Arc<SessionManager>,
    transcript_handler: LiveVoiceTranscriptHandler,
    call_ended_handler: LiveVoiceCallEndedHandler,
    delegation_handler: LiveVoiceDelegationHandler,
) {
    tokio::spawn(async move {
        let session_id = call.session_id().to_string();
        let exit = run_live_call(
            &mut call,
            stop_requested,
            &session_id,
            &session_manager,
            transcript_handler.as_ref(),
            delegation_handler,
        )
        .await;
        let call_id = call.id().clone();
        let control = remove_matching_call(&calls_by_session, &session_id, &call_id);
        let run_guard = control.map(|control| control.run_guard);
        if let Some(finalization) = exit.background_finalization {
            let manager = session_manager.clone();
            let finalization_session_id = session_id.clone();
            tokio::spawn(async move {
                let _run_guard = run_guard;
                finalization
                    .finish(&manager, &finalization_session_id)
                    .await;
            });
        } else {
            drop(run_guard);
        }
        completion_tx.send_replace(Some(exit.completion));
        call_ended_handler(LiveVoiceCallEnded {
            session_id,
            call_id,
            completion: exit.completion,
        });
    });
}

async fn run_live_call(
    call: &mut LiveVoiceCall,
    stop_requested: CancellationToken,
    session_id: &str,
    session_manager: &SessionManager,
    transcript_handler: &(dyn Fn(Message) + Send + Sync),
    delegation_handler: LiveVoiceDelegationHandler,
) -> LiveCallExit {
    let mut stopping = false;
    let mut pending_delegation: Option<PendingDelegation> = None;
    let mut deferred_transcript = Vec::new();
    loop {
        let event = if stopping {
            call.next_provider_event().await
        } else {
            tokio::select! {
                biased;
                _ = stop_requested.cancelled() => {
                    match timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await {
                        Ok(Ok(())) => {
                            stopping = true;
                            continue;
                        }
                        _ => {
                            return finish_live_call(
                                call,
                                session_manager,
                                session_id,
                                pending_delegation.take(),
                                &mut deferred_transcript,
                                LiveVoiceCallCompletion::Failed,
                            )
                            .await;
                        }
                    }
                }
                result = async {
                    let pending = pending_delegation
                        .as_mut()
                        .expect("delegation is pending");
                    pending.future.as_mut().await
                }, if pending_delegation.is_some() => {
                    let pending = pending_delegation.take().expect("delegation is pending");
                    defer_transcript(&mut deferred_transcript, call.take_transcript());
                    let pending_context = call.pending_transcript_context();
                    if persist_running_transcript(
                        session_manager,
                        session_id,
                        std::mem::take(&mut deferred_transcript),
                        pending_context,
                    )
                    .await
                    .is_err()
                    {
                        let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        return LiveCallExit::failed();
                    }
                    if call
                        .send_delegation_update(
                            pending.provider_delegation_id,
                            bound_delegation_update(result).await,
                        )
                        .await
                        .is_err()
                    {
                        let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        return LiveCallExit::failed();
                    }
                    continue;
                }
                event = call.next_provider_event() => event,
            }
        };

        match event {
            ProviderConnectionEvent::TranscriptDelta {
                event_id,
                role,
                text,
                end_ms,
                ..
            } => {
                if let Some((finalized, delta)) =
                    call.observe_transcript(event_id, role, &text, end_ms)
                {
                    transcript_handler(delta);
                    if pending_delegation.is_some() {
                        defer_transcript(&mut deferred_transcript, finalized);
                    } else if persist_transcript(session_manager, session_id, finalized)
                        .await
                        .is_err()
                    {
                        if !stopping {
                            let _ =
                                timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        }
                        return LiveCallExit::failed();
                    }
                }
            }
            ProviderConnectionEvent::DelegationRequested {
                event_id,
                delegation_id,
                offset_ms,
            } => match call.delegation_input(event_id, delegation_id.clone(), offset_ms) {
                DelegationDecision::Ignore => {}
                DelegationDecision::Reject(text) => {
                    if call
                        .send_delegation_update(delegation_id, bound_delegation_update(text).await)
                        .await
                        .is_err()
                    {
                        let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        return finish_live_call(
                            call,
                            session_manager,
                            session_id,
                            pending_delegation.take(),
                            &mut deferred_transcript,
                            LiveVoiceCallCompletion::Failed,
                        )
                        .await;
                    }
                }
                DelegationDecision::Accept(input) => {
                    if pending_delegation.is_some() {
                        let result = delegation_handler(
                            session_id.to_string(),
                            LiveVoiceDelegationCommand::Steer { input },
                        )
                        .await;
                        call.accept_delegation(offset_ms);
                        if call
                            .send_delegation_update(
                                delegation_id,
                                bound_delegation_update(result).await,
                            )
                            .await
                            .is_err()
                        {
                            let _ =
                                timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                            return finish_live_call(
                                call,
                                session_manager,
                                session_id,
                                pending_delegation.take(),
                                &mut deferred_transcript,
                                LiveVoiceCallCompletion::Failed,
                            )
                            .await;
                        }
                        continue;
                    }
                    if persist_transcript(session_manager, session_id, call.take_transcript())
                        .await
                        .is_err()
                    {
                        if !stopping {
                            let _ =
                                timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        }
                        return LiveCallExit::failed();
                    }
                    let future = delegation_handler(
                        session_id.to_string(),
                        LiveVoiceDelegationCommand::Start {
                            input: DELEGATION_INSTRUCTION.into(),
                        },
                    );
                    pending_delegation = Some(PendingDelegation {
                        provider_delegation_id: delegation_id,
                        future,
                    });
                    call.accept_delegation(offset_ms);
                    call.clear_pending_transcript();
                }
            },
            ProviderConnectionEvent::Closed => {
                let completion = if stopping {
                    LiveVoiceCallCompletion::Stopped
                } else {
                    LiveVoiceCallCompletion::Failed
                };
                return finish_live_call(
                    call,
                    session_manager,
                    session_id,
                    pending_delegation.take(),
                    &mut deferred_transcript,
                    completion,
                )
                .await;
            }
            ProviderConnectionEvent::ReceiverLagged | ProviderConnectionEvent::Failed => {
                if !stopping {
                    let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                }
                return finish_live_call(
                    call,
                    session_manager,
                    session_id,
                    pending_delegation.take(),
                    &mut deferred_transcript,
                    LiveVoiceCallCompletion::Failed,
                )
                .await;
            }
        }
    }
}

impl LiveCallExit {
    fn failed() -> Self {
        Self {
            completion: LiveVoiceCallCompletion::Failed,
            background_finalization: None,
        }
    }
}

impl BackgroundFinalization {
    async fn finish(self, session_manager: &SessionManager, session_id: &str) {
        let Self {
            delegation,
            deferred_transcript,
            pending_context,
        } = self;
        let _ = delegation.await;
        let _ = persist_running_transcript(
            session_manager,
            session_id,
            deferred_transcript,
            pending_context,
        )
        .await;
    }
}

async fn finish_live_call(
    call: &mut LiveVoiceCall,
    session_manager: &SessionManager,
    session_id: &str,
    pending_delegation: Option<PendingDelegation>,
    deferred_transcript: &mut Vec<Message>,
    completion: LiveVoiceCallCompletion,
) -> LiveCallExit {
    if let Some(pending_delegation) = pending_delegation {
        defer_transcript(deferred_transcript, call.take_transcript());
        return LiveCallExit {
            completion,
            background_finalization: Some(BackgroundFinalization {
                delegation: pending_delegation.future,
                deferred_transcript: std::mem::take(deferred_transcript),
                pending_context: call.pending_transcript_context(),
            }),
        };
    }

    let persisted = persist_transcript(session_manager, session_id, call.take_transcript())
        .await
        .is_ok();
    LiveCallExit {
        completion: if persisted {
            completion
        } else {
            LiveVoiceCallCompletion::Failed
        },
        background_finalization: None,
    }
}

fn defer_transcript(deferred_transcript: &mut Vec<Message>, message: Option<Message>) {
    if let Some(message) = message {
        deferred_transcript.push(message.user_only());
    }
}

async fn persist_running_transcript(
    session_manager: &SessionManager,
    session_id: &str,
    deferred_transcript: Vec<Message>,
    pending_context: Option<String>,
) -> anyhow::Result<()> {
    for message in deferred_transcript {
        session_manager.add_message(session_id, &message).await?;
    }
    if let Some(context) = pending_context {
        session_manager
            .add_message(
                session_id,
                &Message::user()
                    .with_id(format!("msg_live_context_{}", Uuid::now_v7()))
                    .with_text(context)
                    .agent_only(),
            )
            .await?;
    }
    Ok(())
}

async fn bound_delegation_update(text: String) -> String {
    let Ok(counter) = TokenCounter::new().await else {
        return text;
    };
    if counter.count_tokens(&text) <= DELEGATION_UPDATE_TOKEN_LIMIT {
        return text;
    }

    let allowed = DELEGATION_UPDATE_TOKEN_LIMIT - counter.count_tokens(SAVED_RESULT_NOTICE);
    let boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut low = 0;
    let mut high = boundaries.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let end = boundaries.get(middle).copied().unwrap_or(text.len());
        if counter.count_tokens(&text[..end]) <= allowed {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let end = boundaries.get(low).copied().unwrap_or(text.len());
    format!("{}{}", text[..end].trim_end(), SAVED_RESULT_NOTICE)
}

async fn persist_transcript(
    session_manager: &SessionManager,
    session_id: &str,
    message: Option<Message>,
) -> anyhow::Result<()> {
    if let Some(message) = message {
        session_manager.add_message(session_id, &message).await?;
    }
    Ok(())
}

fn remove_matching_call(
    calls_by_session: &LiveCallControls,
    session_id: &str,
    call_id: &LiveVoiceCallId,
) -> Option<LiveCallControl> {
    let mut calls = calls_by_session.lock().expect("live voice lock poisoned");
    if matches!(
        calls.get(session_id),
        Some(control) if &control.call_id == call_id
    ) {
        calls.remove(session_id)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
