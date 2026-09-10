use super::call::{LiveVoiceCall, LiveVoiceCallId, LiveVoiceCallState};
use crate::acp::server::ActiveRunRegistry;
use crate::config::GooseMode;
use goose_providers::live_voice_provider::{
    LiveVoiceProvider, LiveVoiceProviderAvailability, ProviderConnectionEvent,
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

type LiveCallControls = Arc<StdMutex<HashMap<String, LiveCallControl>>>;
pub(super) type LiveVoiceCallEndedHandler = Arc<dyn Fn(LiveVoiceCallEnded) + Send + Sync>;

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
        mode: GooseMode,
        offer: WebRtcOffer,
        call_ended_handler: LiveVoiceCallEndedHandler,
    ) -> Result<StartLiveVoiceCallResult, LiveVoiceError> {
        if self.provider.availability() != LiveVoiceProviderAvailability::Ready
            || mode != GooseMode::Auto
        {
            return Err(LiveVoiceError::Unavailable);
        }
        let run_guard = LiveRunGuard::start(self.active_runs.clone(), session_id)
            .ok_or(LiveVoiceError::Unavailable)?;
        let (answer, provider_connection) = self
            .provider
            .start(offer)
            .await
            .map_err(|_| LiveVoiceError::StartFailed)?;

        let call_id = LiveVoiceCallId::new();
        let call = LiveVoiceCall::new(call_id.clone(), provider_connection);
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
            session_id.to_string(),
            call,
            stop_requested,
            finished_state_tx,
            call_ended_handler,
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
    session_id: String,
    mut call: LiveVoiceCall,
    stop_requested: CancellationToken,
    finished_state_tx: watch::Sender<Option<LiveVoiceCallState>>,
    call_ended_handler: LiveVoiceCallEndedHandler,
) {
    tokio::spawn(async move {
        let final_state = run_live_call(&mut call, stop_requested).await;
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
) -> LiveVoiceCallState {
    tokio::select! {
        biased;
        _ = stop_requested.cancelled() => {
            match timeout(PROVIDER_CLEANUP_TIMEOUT, call.stop()).await {
                Ok(state) => state,
                Err(_) => call.fail(),
            }
        }
        event = call.next_provider_event() => {
            let state = call.observe_provider_event(event);
            if event == ProviderConnectionEvent::Failed {
                let _ = timeout(PROVIDER_CLEANUP_TIMEOUT, call.cleanup_provider()).await;
            }
            state
        }
    }
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
    use tokio::task::JoinHandle;

    type StartResult = Result<StartLiveVoiceCallResult, LiveVoiceError>;

    fn ignore_call_ended() -> LiveVoiceCallEndedHandler {
        Arc::new(|_| {})
    }

    fn service(availability: LiveVoiceProviderAvailability) -> LiveVoiceService {
        let (provider, _starts) = provider_channel_with_availability(availability);
        LiveVoiceService::new(provider, Arc::new(ActiveRunRegistry::default()))
    }

    fn spawn_start(
        service: Arc<LiveVoiceService>,
        session_id: &'static str,
        offer: &'static str,
    ) -> JoinHandle<StartResult> {
        tokio::spawn(async move {
            service
                .start_call(
                    session_id,
                    GooseMode::Auto,
                    WebRtcOffer::new(offer.into()).unwrap(),
                    ignore_call_ended(),
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

    async fn establish_call() -> (Arc<LiveVoiceService>, FakeConnectionDriver, LiveVoiceCallId) {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let start_task = spawn_start(service.clone(), "main-session", "offer");
        let connection = starts
            .recv()
            .await
            .unwrap()
            .accept(WebRtcAnswer::new("answer".into()).unwrap())
            .unwrap();
        let call_id = start_task.await.unwrap().unwrap().call_id;
        (service, connection, call_id)
    }

    fn finished_state_receiver(
        service: &LiveVoiceService,
    ) -> watch::Receiver<Option<LiveVoiceCallState>> {
        service
            .calls_by_session
            .lock()
            .unwrap()
            .get("main-session")
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

    #[tokio::test]
    async fn a_start_reserves_the_session_until_it_finishes() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let first = spawn_start(service.clone(), "main-session", "first-offer");
        let pending = starts.recv().await.unwrap();

        assert_availability(&service, LiveVoiceAvailability::ChatBusy);

        let second = service
            .start_call(
                "main-session",
                GooseMode::Auto,
                WebRtcOffer::new("second-offer".into()).unwrap(),
                ignore_call_ended(),
            )
            .await;
        assert!(matches!(second, Err(LiveVoiceError::Unavailable)));

        pending.reject("failed").unwrap();
        assert!(matches!(
            first.await.unwrap(),
            Err(LiveVoiceError::StartFailed)
        ));
        assert_availability(&service, LiveVoiceAvailability::Ready);
    }

    #[tokio::test]
    async fn a_cancelled_start_releases_the_session() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(
            provider,
            Arc::new(ActiveRunRegistry::default()),
        ));
        let start_task = spawn_start(service.clone(), "main-session", "offer");
        let pending = starts.recv().await.unwrap();

        start_task.abort();
        assert!(matches!(start_task.await, Err(error) if error.is_cancelled()));
        assert_availability(&service, LiveVoiceAvailability::Ready);
        drop(pending);
    }

    #[tokio::test]
    async fn stop_failure_releases_the_session() {
        let (service, mut connection, call_id) = establish_call().await;
        let stop_service = service.clone();
        let stop =
            tokio::spawn(async move { stop_service.stop_call("main-session", &call_id).await });
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
        assert_availability(&service, LiveVoiceAvailability::Ready);
    }

    #[tokio::test]
    async fn repeated_stop_uses_one_provider_shutdown() {
        let (service, mut connection, call_id) = establish_call().await;
        let first = service.stop_call("main-session", &call_id);
        let second = service.stop_call("main-session", &call_id);
        let provider = async move {
            connection
                .next_stop_request()
                .await
                .unwrap()
                .send(Ok(()))
                .unwrap();
            assert!(connection.next_stop_request().await.is_none());
        };

        let (first, second, ()) = tokio::join!(first, second, provider);

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_availability(&service, LiveVoiceAvailability::Ready);
    }

    #[tokio::test]
    async fn provider_terminal_events_fail_and_release_the_session() {
        for event in [
            ProviderConnectionEvent::Closed,
            ProviderConnectionEvent::Failed,
        ] {
            let (service, mut connection, call_id) = establish_call().await;
            let finished_state_rx = finished_state_receiver(&service);
            connection.send_event(event).unwrap();
            if event == ProviderConnectionEvent::Failed {
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
                service.stop_call("main-session", &call_id).await,
                Err(LiveVoiceError::Unavailable)
            ));
            assert_availability(&service, LiveVoiceAvailability::Ready);
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
        let start_task = tokio::spawn(async move {
            start_service
                .start_call(
                    "main-session",
                    GooseMode::Auto,
                    WebRtcOffer::new("offer".into()).unwrap(),
                    call_ended_handler,
                )
                .await
        });
        let mut connection = starts
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

        assert_eq!(ended.session_id, "main-session");
        assert_eq!(ended.call_id, call_id);
        assert_eq!(ended.state, LiveVoiceCallState::Failed);
        assert_availability(&service, LiveVoiceAvailability::Ready);
        assert!(ended_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn cleanup_timeout_fails_and_releases_the_session() {
        let (service, mut connection, call_id) = establish_call().await;
        let stop_service = service.clone();
        let stop =
            tokio::spawn(async move { stop_service.stop_call("main-session", &call_id).await });
        let pending_response = connection.next_stop_request().await.unwrap();
        tokio::time::advance(PROVIDER_CLEANUP_TIMEOUT).await;

        assert!(matches!(
            stop.await.unwrap(),
            Err(LiveVoiceError::StopFailed)
        ));
        assert_availability(&service, LiveVoiceAvailability::Ready);
        drop(pending_response);
    }

    #[tokio::test]
    async fn queued_stop_wins_a_provider_close_race() {
        let (service, mut connection, call_id) = establish_call().await;
        connection
            .send_event(ProviderConnectionEvent::Closed)
            .unwrap();

        let stop = service.stop_call("main-session", &call_id);
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
        assert_availability(&service, LiveVoiceAvailability::Ready);
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
