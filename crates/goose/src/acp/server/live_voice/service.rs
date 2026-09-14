use super::call::{DelegationInput, LiveVoiceCall, LiveVoiceCallId, LiveVoiceCallState};
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
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::{sync::watch, time::timeout};
use tokio_util::sync::CancellationToken;

const PROVIDER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_VOICE_INPUT_MESSAGE_COUNT: usize = 10;
const DELEGATION_UPDATE_TOKEN_LIMIT: usize = 500;
const SAVED_RESULT_NOTICE: &str = "\n\nThe full result is saved in Goose.";

type LiveCallControls = Arc<StdMutex<HashMap<String, LiveCallControl>>>;
pub(super) type LiveVoiceCallEndedHandler = Arc<dyn Fn(LiveVoiceCallEnded) + Send + Sync>;
pub(super) type LiveVoiceTranscriptHandler = Arc<dyn Fn(Message) + Send + Sync>;
pub(super) type LiveVoiceDelegationHandler =
    Arc<dyn Fn(String, String, CancellationToken) -> BoxFuture<'static, String> + Send + Sync>;

struct PendingDelegation {
    provider_delegation_id: String,
    cancellation: CancellationToken,
    future: BoxFuture<'static, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiveVoiceCallEnded {
    pub(super) session_id: String,
    pub(super) call_id: LiveVoiceCallId,
    pub(super) state: LiveVoiceCallState,
}

pub(super) struct StartLiveVoiceCallResult {
    pub(super) call_id: LiveVoiceCallId,
    pub(super) answer: WebRtcAnswer,
    pub(super) finished_state_rx: watch::Receiver<Option<LiveVoiceCallState>>,
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
    _run_guard: LiveRunGuard,
    call_id: LiveVoiceCallId,
    stop_requested: CancellationToken,
    finished_state_rx: watch::Receiver<Option<LiveVoiceCallState>>,
}

impl LiveCallControl {
    fn request_stop(&self) -> watch::Receiver<Option<LiveVoiceCallState>> {
        self.stop_requested.cancel();
        self.finished_state_rx.clone()
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
            calls_by_session: Arc::new(StdMutex::new(HashMap::new())),
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
            .start(offer, input_messages.clone())
            .await
            .map_err(|_| LiveVoiceError::StartFailed)?;

        let call_id = LiveVoiceCallId::new();
        let call = LiveVoiceCall::new(
            session_id.to_string(),
            call_id.clone(),
            provider_connection,
            input_messages,
        );
        let stop_requested = CancellationToken::new();
        let (finished_state_tx, finished_state_rx) = watch::channel(None);
        let mut calls = self
            .calls_by_session
            .lock()
            .expect("live voice lock poisoned");
        calls.insert(
            session_id.to_string(),
            LiveCallControl {
                _run_guard: run_guard,
                call_id: call_id.clone(),
                stop_requested: stop_requested.clone(),
                finished_state_rx: finished_state_rx.clone(),
            },
        );
        drop(calls);
        spawn_live_call(
            self.calls_by_session.clone(),
            call,
            stop_requested,
            finished_state_tx,
            session_manager,
            transcript_handler,
            call_ended_handler,
            delegation_handler,
        );
        Ok(StartLiveVoiceCallResult {
            call_id,
            answer,
            finished_state_rx,
        })
    }

    pub(super) async fn stop_call(
        &self,
        session_id: &str,
        call_id: &LiveVoiceCallId,
    ) -> Result<(), LiveVoiceError> {
        let finished_state_rx = {
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
        let final_state = wait_until_finished(finished_state_rx).await?;
        match final_state {
            LiveVoiceCallState::Stopped => Ok(()),
            LiveVoiceCallState::Failed => Err(LiveVoiceError::StopFailed),
            LiveVoiceCallState::Live => Err(LiveVoiceError::StopFailed),
        }
    }
}

fn live_voice_input_messages(conversation: &Conversation) -> Vec<LiveVoiceInputMessage> {
    let messages = conversation
        .messages()
        .iter()
        .filter(|message| message.is_user_visible() || is_delegated_outcome(message))
        .into_iter()
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

fn is_delegated_outcome(message: &Message) -> bool {
    message
        .metadata
        .operation_note("live_delegation", "outcome")
        .is_some_and(|value| value.as_bool() == Some(true))
}

pub(super) async fn wait_until_finished(
    mut finished_state_rx: watch::Receiver<Option<LiveVoiceCallState>>,
) -> Result<LiveVoiceCallState, LiveVoiceError> {
    loop {
        if let Some(state) = *finished_state_rx.borrow() {
            return Ok(state);
        }
        finished_state_rx
            .changed()
            .await
            .map_err(|_| LiveVoiceError::StopFailed)?;
    }
}

fn spawn_live_call(
    calls_by_session: LiveCallControls,
    mut call: LiveVoiceCall,
    stop_requested: CancellationToken,
    finished_state_tx: watch::Sender<Option<LiveVoiceCallState>>,
    session_manager: Arc<SessionManager>,
    transcript_handler: LiveVoiceTranscriptHandler,
    call_ended_handler: LiveVoiceCallEndedHandler,
    delegation_handler: LiveVoiceDelegationHandler,
) {
    tokio::spawn(async move {
        let session_id = call.session_id().to_string();
        let final_state = run_live_call(
            &mut call,
            stop_requested,
            &session_id,
            &session_manager,
            transcript_handler.as_ref(),
            delegation_handler,
        )
        .await;
        let call_id = call.id().clone();
        remove_matching_call(&calls_by_session, &session_id, &call_id);
        finished_state_tx.send_replace(Some(final_state));
        call_ended_handler(LiveVoiceCallEnded {
            session_id,
            call_id,
            state: final_state,
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
) -> LiveVoiceCallState {
    let mut stopping = false;
    let mut pending_delegation: Option<PendingDelegation> = None;
    loop {
        let event = if stopping {
            call.next_provider_event().await
        } else {
            tokio::select! {
                biased;
                _ = stop_requested.cancelled() => {
                    cancel_delegation(&mut pending_delegation).await;
                    match timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await {
                        Ok(Ok(())) => {
                            stopping = true;
                            continue;
                        }
                        _ => {
                            let _ = persist_transcript(
                                session_manager,
                                session_id,
                                call.take_transcript(),
                            )
                            .await;
                            return call.fail();
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
                    if call
                        .send_delegation_update(
                            pending.provider_delegation_id,
                            bound_delegation_update(result).await,
                        )
                        .await
                        .is_err()
                    {
                        cancel_delegation(&mut pending_delegation).await;
                        let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        let _ = persist_transcript(
                            session_manager,
                            session_id,
                            call.take_transcript(),
                        )
                        .await;
                        return call.fail();
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
                start_ms,
                end_ms,
            } => {
                if let Some((finalized, delta)) =
                    call.observe_transcript(event_id, role, &text, start_ms, end_ms)
                {
                    transcript_handler(delta);
                    if persist_transcript(session_manager, session_id, finalized)
                        .await
                        .is_err()
                    {
                        cancel_delegation(&mut pending_delegation).await;
                        if !stopping {
                            let _ =
                                timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        }
                        return call.fail();
                    }
                }
            }
            ProviderConnectionEvent::DelegationRequested {
                event_id,
                delegation_id,
                offset_ms,
            } => match call.accept_delegation(event_id, delegation_id.clone(), offset_ms) {
                DelegationInput::Ignore => {}
                DelegationInput::Reject(text) => {
                    if call
                        .send_delegation_update(delegation_id, bound_delegation_update(text).await)
                        .await
                        .is_err()
                    {
                        let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                        let _ =
                            persist_transcript(session_manager, session_id, call.take_transcript())
                                .await;
                        return call.fail();
                    }
                }
                DelegationInput::Accept(input) => {
                    let cancellation = CancellationToken::new();
                    let future =
                        delegation_handler(session_id.to_string(), input, cancellation.clone());
                    pending_delegation = Some(PendingDelegation {
                        provider_delegation_id: delegation_id,
                        cancellation,
                        future,
                    });
                }
            },
            ProviderConnectionEvent::Closed => {
                cancel_delegation(&mut pending_delegation).await;
                let persisted =
                    persist_transcript(session_manager, session_id, call.take_transcript())
                        .await
                        .is_ok();
                return if stopping && persisted {
                    call.finish_stop()
                } else {
                    call.fail()
                };
            }
            ProviderConnectionEvent::ReceiverLagged => {
                cancel_delegation(&mut pending_delegation).await;
                let _ =
                    persist_transcript(session_manager, session_id, call.take_transcript()).await;
                if !stopping {
                    let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                }
                return call.fail();
            }
            ProviderConnectionEvent::Failed => {
                cancel_delegation(&mut pending_delegation).await;
                let _ =
                    persist_transcript(session_manager, session_id, call.take_transcript()).await;
                if !stopping {
                    let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
                }
                return call.fail();
            }
        }
    }
}

async fn cancel_delegation(pending: &mut Option<PendingDelegation>) {
    let Some(PendingDelegation {
        cancellation,
        future,
        ..
    }) = pending.take()
    else {
        return;
    };
    cancellation.cancel();
    let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, future).await;
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
mod tests {
    use super::*;
    use goose_providers::live_voice_provider::fake::{
        provider_channel, provider_channel_with_availability, FakeConnectionDriver,
    };
    use goose_providers::model::ModelConfig;
    use rmcp::model::Role;
    use tokio::task::JoinHandle;

    type StartResult = Result<StartLiveVoiceCallResult, LiveVoiceError>;

    fn ignore_call_ended() -> LiveVoiceCallEndedHandler {
        Arc::new(|_| {})
    }

    fn ignore_transcript() -> LiveVoiceTranscriptHandler {
        Arc::new(|_| {})
    }

    fn ignore_delegation() -> LiveVoiceDelegationHandler {
        Arc::new(|_, _, _| Box::pin(async { "unused".to_string() }))
    }

    fn session_manager() -> Arc<SessionManager> {
        Arc::new(SessionManager::new(tempfile::tempdir().unwrap().keep()))
    }

    fn service(availability: LiveVoiceProviderAvailability) -> LiveVoiceService {
        let (provider, _starts) = provider_channel_with_availability(availability);
        LiveVoiceService::new(provider, Arc::new(ActiveRunRegistry::default()))
    }

    async fn live_session(
        conversation: impl IntoIterator<Item = Message>,
    ) -> (Arc<SessionManager>, String) {
        let manager = session_manager();
        let session = manager
            .create_session(
                std::path::PathBuf::from("/tmp/test"),
                "Live voice".into(),
                crate::session::session_manager::SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        manager
            .update(&session.id)
            .provider_name("test")
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();
        for message in conversation {
            manager.add_message(&session.id, &message).await.unwrap();
        }
        (manager, session.id)
    }

    fn spawn_start(
        service: Arc<LiveVoiceService>,
        session_id: String,
        offer: &'static str,
        session_manager: Arc<SessionManager>,
    ) -> JoinHandle<StartResult> {
        tokio::spawn(async move {
            service
                .start_call(
                    &session_id,
                    WebRtcOffer::new(offer.into()).unwrap(),
                    session_manager,
                    ignore_transcript(),
                    ignore_call_ended(),
                    ignore_delegation(),
                )
                .await
        })
    }

    fn assert_availability(service: &LiveVoiceService, expected: LiveVoiceAvailability) {
        assert_eq!(
            service.availability("main-session", GooseMode::Auto),
            expected
        );
    }

    async fn establish_call() -> (
        Arc<LiveVoiceService>,
        FakeConnectionDriver,
        LiveVoiceCallId,
        String,
    ) {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let (manager, session_id) =
            live_session([Message::user().with_text("prior context")]).await;
        let start_task = spawn_start(service.clone(), session_id.clone(), "offer", manager);
        let connection = starts
            .recv()
            .await
            .unwrap()
            .accept(WebRtcAnswer::new("answer".into()).unwrap())
            .unwrap();
        let call_id = start_task.await.unwrap().unwrap().call_id;
        (service, connection, call_id, session_id)
    }

    fn finished_state_receiver(
        service: &LiveVoiceService,
        session_id: &str,
    ) -> watch::Receiver<Option<LiveVoiceCallState>> {
        service
            .calls_by_session
            .lock()
            .unwrap()
            .get(session_id)
            .unwrap()
            .finished_state_rx
            .clone()
    }

    #[test]
    fn reports_each_eligibility_gate() {
        assert_availability(
            &service(LiveVoiceProviderAvailability::Disabled),
            LiveVoiceAvailability::FeatureDisabled,
        );
        assert_availability(
            &service(LiveVoiceProviderAvailability::Unavailable),
            LiveVoiceAvailability::ProviderUnavailable,
        );

        let ready = service(LiveVoiceProviderAvailability::Ready);
        let run_guard = LiveRunGuard::start(ready.active_runs.clone(), "main-session").unwrap();
        assert_eq!(
            ready.availability("main-session", GooseMode::Auto),
            LiveVoiceAvailability::ChatBusy
        );
        drop(run_guard);
        assert_eq!(
            ready.availability("main-session", GooseMode::Approve),
            LiveVoiceAvailability::RequiresAutonomousMode
        );
        assert_availability(&ready, LiveVoiceAvailability::Ready);
    }

    #[test]
    fn input_messages_are_the_latest_visible_non_empty_text() {
        let mut messages = vec![
            Message::user().with_text("hidden").agent_only(),
            Message::assistant().with_thinking("internal", "signature"),
            Message::user().with_text(" "),
        ];
        messages.extend((0..12).map(|index| {
            if index % 2 == 0 {
                Message::user().with_text(format!("message {index}"))
            } else {
                Message::assistant().with_text(format!("message {index}"))
            }
        }));
        let mut outcome_metadata = crate::conversation::message::MessageMetadata::agent_only();
        outcome_metadata.set_operation_note(
            "live_delegation",
            "outcome",
            serde_json::Value::Bool(true),
        );
        messages.push(
            Message::assistant()
                .with_text("delegated result")
                .with_metadata(outcome_metadata),
        );
        messages.push(Message::assistant().with_text("other hidden").agent_only());
        let conversation = Conversation::new_unvalidated(messages);

        let input_messages = live_voice_input_messages(&conversation);
        assert_eq!(input_messages.len(), LIVE_VOICE_INPUT_MESSAGE_COUNT);
        assert_eq!(input_messages.first().unwrap().text, "message 3");
        assert_eq!(input_messages.first().unwrap().role, Role::Assistant);
        assert_eq!(input_messages.last().unwrap().text, "delegated result");
        assert_eq!(input_messages.last().unwrap().role, Role::Assistant);
        assert!(!input_messages
            .iter()
            .any(|message| message.text == "other hidden"));
    }

    #[tokio::test]
    async fn a_start_reserves_the_session_until_it_finishes() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let (manager, session_id) =
            live_session([Message::user().with_text("prior context")]).await;
        let first = spawn_start(
            service.clone(),
            session_id.clone(),
            "first-offer",
            manager.clone(),
        );
        let pending = starts.recv().await.unwrap();

        assert_eq!(
            pending.input_messages,
            vec![LiveVoiceInputMessage {
                role: Role::User,
                text: "prior context".into(),
            }]
        );

        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::ChatBusy
        );

        let second = service
            .start_call(
                &session_id,
                WebRtcOffer::new("second-offer".into()).unwrap(),
                manager,
                ignore_transcript(),
                ignore_call_ended(),
                ignore_delegation(),
            )
            .await;
        assert!(matches!(second, Err(LiveVoiceError::Unavailable)));

        pending.reject("failed").unwrap();
        assert!(matches!(
            first.await.unwrap(),
            Err(LiveVoiceError::StartFailed)
        ));
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
    }

    #[tokio::test]
    async fn a_cancelled_start_releases_the_session() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let (manager, session_id) = live_session([]).await;
        let start_task = spawn_start(service.clone(), session_id.clone(), "offer", manager);
        let pending = starts.recv().await.unwrap();

        start_task.abort();
        assert!(matches!(start_task.await, Err(error) if error.is_cancelled()));
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
        drop(pending);
    }

    #[tokio::test]
    async fn stop_failure_releases_the_session() {
        let (service, mut connection, call_id, session_id) = establish_call().await;
        let stop_service = service.clone();
        let stop_session_id = session_id.clone();
        let stop =
            tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
        connection
            .next_stop_request()
            .await
            .unwrap()
            .send(Err("failed".into()))
            .unwrap();

        assert!(matches!(
            stop.await.unwrap(),
            Err(LiveVoiceError::StopFailed)
        ));
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
    }

    #[tokio::test]
    async fn repeated_stop_uses_one_provider_shutdown() {
        let (service, mut connection, call_id, session_id) = establish_call().await;
        let first = service.stop_call(&session_id, &call_id);
        let second = service.stop_call(&session_id, &call_id);
        let provider = async move {
            connection
                .next_stop_request()
                .await
                .unwrap()
                .send(Ok(()))
                .unwrap();
            connection
                .send_event(ProviderConnectionEvent::Closed)
                .unwrap();
            assert!(connection.next_stop_request().await.is_none());
        };

        let (first, second, ()) = tokio::join!(first, second, provider);

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
    }

    #[tokio::test]
    async fn transcript_is_projected_live_and_persisted_after_stop_draining() {
        let temp_dir = tempfile::tempdir().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let session = session_manager
            .create_session(
                std::path::PathBuf::from("/tmp/test"),
                "Live transcript".into(),
                crate::session::session_manager::SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        session_manager
            .update(&session.id)
            .provider_name("test")
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let (transcript_tx, mut transcript_rx) = tokio::sync::mpsc::unbounded_channel();
        let transcript_handler: LiveVoiceTranscriptHandler = Arc::new(move |message| {
            transcript_tx.send(message).unwrap();
        });
        let start_service = service.clone();
        let start_session_id = session.id.clone();
        let start_manager = session_manager.clone();
        let start = tokio::spawn(async move {
            start_service
                .start_call(
                    &start_session_id,
                    WebRtcOffer::new("offer".into()).unwrap(),
                    start_manager,
                    transcript_handler,
                    ignore_call_ended(),
                    ignore_delegation(),
                )
                .await
        });
        let mut connection = starts
            .recv()
            .await
            .unwrap()
            .accept(WebRtcAnswer::new("answer".into()).unwrap())
            .unwrap();
        let call_id = start.await.unwrap().unwrap().call_id;
        let delta = |event_id: &str, text: &str| ProviderConnectionEvent::TranscriptDelta {
            event_id: event_id.into(),
            role: Role::User,
            text: text.into(),
            start_ms: 0,
            end_ms: 1,
        };

        connection.send_event(delta("1", "hello")).unwrap();
        assert_eq!(
            transcript_rx.recv().await.unwrap().as_concat_text(),
            "hello"
        );
        connection
            .send_event(ProviderConnectionEvent::DelegationRequested {
                event_id: "delegation-event".into(),
                delegation_id: "delegation".into(),
                offset_ms: 1,
            })
            .unwrap();
        assert_eq!(
            connection.next_delegation_update().await.unwrap().text,
            "unused"
        );
        assert!(session_manager
            .get_session(&session.id, true)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages()
            .is_empty());

        let stop_service = service.clone();
        let stop_session_id = session.id.clone();
        let stop =
            tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
        let stop_response = connection.next_stop_request().await.unwrap();
        connection.send_event(delta("2", " world")).unwrap();
        stop_response.send(Ok(())).unwrap();
        connection
            .send_event(ProviderConnectionEvent::Closed)
            .unwrap();

        let revised = transcript_rx.recv().await.unwrap();
        assert_eq!(revised.as_concat_text(), " world");
        assert!(stop.await.unwrap().is_ok());

        let stored = session_manager
            .get_session(&session.id, true)
            .await
            .unwrap();
        let messages = stored.conversation.unwrap();
        assert_eq!(messages.messages().len(), 1);
        assert_eq!(messages.messages()[0].as_concat_text(), "hello world");
    }

    #[tokio::test]
    async fn provider_terminal_events_fail_and_release_the_session() {
        for event in [
            ProviderConnectionEvent::Closed,
            ProviderConnectionEvent::Failed,
        ] {
            let (service, mut connection, call_id, session_id) = establish_call().await;
            let finished_state_rx = finished_state_receiver(&service, &session_id);
            let requires_cleanup = event == ProviderConnectionEvent::Failed;
            connection.send_event(event).unwrap();
            if requires_cleanup {
                connection
                    .next_stop_request()
                    .await
                    .unwrap()
                    .send(Ok(()))
                    .unwrap();
            }

            assert_eq!(
                wait_until_finished(finished_state_rx).await.unwrap(),
                LiveVoiceCallState::Failed
            );
            assert!(matches!(
                service.stop_call(&session_id, &call_id).await,
                Err(LiveVoiceError::Unavailable)
            ));
            assert_eq!(
                service.availability(&session_id, GooseMode::Auto),
                LiveVoiceAvailability::Ready
            );
        }
    }

    #[tokio::test]
    async fn provider_terminal_publishes_one_call_ended_update_after_release() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let (ended_tx, mut ended_rx) = tokio::sync::mpsc::unbounded_channel();
        let call_ended_handler: LiveVoiceCallEndedHandler = Arc::new(move |ended| {
            ended_tx.send(ended).unwrap();
        });
        let start_service = service.clone();
        let (manager, session_id) = live_session([]).await;
        let start_session_id = session_id.clone();
        let start_task = tokio::spawn(async move {
            start_service
                .start_call(
                    &start_session_id,
                    WebRtcOffer::new("offer".into()).unwrap(),
                    manager,
                    ignore_transcript(),
                    call_ended_handler,
                    ignore_delegation(),
                )
                .await
        });
        let connection = starts
            .recv()
            .await
            .unwrap()
            .accept(WebRtcAnswer::new("answer".into()).unwrap())
            .unwrap();
        let call_id = start_task.await.unwrap().unwrap().call_id;

        connection
            .send_event(ProviderConnectionEvent::Closed)
            .unwrap();
        let ended = ended_rx.recv().await.unwrap();

        assert_eq!(ended.session_id, session_id);
        assert_eq!(ended.call_id, call_id);
        assert_eq!(ended.state, LiveVoiceCallState::Failed);
        assert_eq!(
            service.availability(&ended.session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
        assert!(ended_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn cleanup_timeout_fails_and_releases_the_session() {
        let (service, mut connection, call_id, session_id) = establish_call().await;
        tokio::time::pause();
        let stop_service = service.clone();
        let stop_session_id = session_id.clone();
        let stop =
            tokio::spawn(async move { stop_service.stop_call(&stop_session_id, &call_id).await });
        let pending_response = connection.next_stop_request().await.unwrap();
        tokio::time::advance(PROVIDER_CLEANUP_TIMEOUT).await;

        assert!(matches!(
            stop.await.unwrap(),
            Err(LiveVoiceError::StopFailed)
        ));
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
        drop(pending_response);
    }

    #[tokio::test]
    async fn queued_stop_wins_a_provider_close_race() {
        let (service, mut connection, call_id, session_id) = establish_call().await;
        connection
            .send_event(ProviderConnectionEvent::Closed)
            .unwrap();

        let stop = service.stop_call(&session_id, &call_id);
        let provider = async move {
            connection
                .next_stop_request()
                .await
                .unwrap()
                .send(Ok(()))
                .unwrap();
        };
        let (stop, ()) = tokio::join!(stop, provider);

        assert!(stop.is_ok());
        assert_eq!(
            service.availability(&session_id, GooseMode::Auto),
            LiveVoiceAvailability::Ready
        );
    }

    #[test]
    fn stale_cleanup_cannot_remove_a_later_call() {
        let active_runs = Arc::new(ActiveRunRegistry::default());
        let calls = Arc::new(StdMutex::new(HashMap::new()));
        let current_id = LiveVoiceCallId("current".into());
        let (_, finished_state_rx) = watch::channel(None);
        calls.lock().unwrap().insert(
            "main-session".into(),
            LiveCallControl {
                _run_guard: LiveRunGuard::start(active_runs.clone(), "main-session").unwrap(),
                call_id: current_id.clone(),
                stop_requested: CancellationToken::new(),
                finished_state_rx,
            },
        );

        remove_matching_call(&calls, "main-session", &LiveVoiceCallId("stale".into()));

        assert!(matches!(
            calls.lock().unwrap().get("main-session"),
            Some(control) if control.call_id == current_id
        ));
        assert!(active_runs.is_active("main-session"));
    }
}
