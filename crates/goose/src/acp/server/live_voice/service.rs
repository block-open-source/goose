use super::call::{LiveVoiceCall, LiveVoiceCallId, LiveVoiceCallState};
use crate::config::GooseMode;
use goose_providers::live_voice_provider::{LiveVoiceProvider, LiveVoiceProviderAvailability};
pub(super) use goose_providers::live_voice_provider::{WebRtcAnswer, WebRtcOffer};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::Mutex;

pub(super) struct StartLiveVoiceCallResult {
    pub(super) call_id: LiveVoiceCallId,
    pub(super) answer: WebRtcAnswer,
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

enum LiveVoiceEntry {
    Starting,
    Active(Arc<Mutex<LiveVoiceCall>>),
}

struct StartReservation<'a> {
    calls_by_session: &'a StdMutex<HashMap<String, LiveVoiceEntry>>,
    session_id: String,
    activated: bool,
}

impl StartReservation<'_> {
    fn activate(mut self, call: LiveVoiceCall) -> Result<(), LiveVoiceError> {
        let call = Arc::new(Mutex::new(call));
        {
            let mut calls = self
                .calls_by_session
                .lock()
                .expect("live voice lock poisoned");
            let Some(entry) = calls.get_mut(&self.session_id) else {
                return Err(LiveVoiceError::StartFailed);
            };
            if !matches!(entry, LiveVoiceEntry::Starting) {
                return Err(LiveVoiceError::StartFailed);
            }
            *entry = LiveVoiceEntry::Active(call);
        }
        self.activated = true;
        Ok(())
    }
}

impl Drop for StartReservation<'_> {
    fn drop(&mut self) {
        if !self.activated {
            let mut calls = self
                .calls_by_session
                .lock()
                .expect("live voice lock poisoned");
            if matches!(calls.get(&self.session_id), Some(LiveVoiceEntry::Starting)) {
                calls.remove(&self.session_id);
            }
        }
    }
}

pub struct LiveVoiceService {
    provider: Arc<dyn LiveVoiceProvider>,
    calls_by_session: StdMutex<HashMap<String, LiveVoiceEntry>>,
}

impl LiveVoiceService {
    pub fn new(provider: Arc<dyn LiveVoiceProvider>) -> Self {
        Self {
            provider,
            calls_by_session: StdMutex::new(HashMap::new()),
        }
    }

    pub(super) fn availability(
        &self,
        session_id: &str,
        mode: GooseMode,
        prompt_active: bool,
    ) -> LiveVoiceAvailability {
        use LiveVoiceAvailability::*;

        match self.provider.availability() {
            LiveVoiceProviderAvailability::Disabled => return FeatureDisabled,
            LiveVoiceProviderAvailability::Unavailable => return ProviderUnavailable,
            LiveVoiceProviderAvailability::Ready => {}
        }

        if prompt_active
            || self
                .calls_by_session
                .lock()
                .expect("live voice lock poisoned")
                .contains_key(session_id)
        {
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
        prompt_active: bool,
        offer: WebRtcOffer,
    ) -> Result<StartLiveVoiceCallResult, LiveVoiceError> {
        if self.availability(session_id, mode, prompt_active) != LiveVoiceAvailability::Ready {
            return Err(LiveVoiceError::Unavailable);
        }

        let reservation = self.reserve(session_id)?;

        let (answer, provider_connection) = self
            .provider
            .start(offer)
            .await
            .map_err(|_| LiveVoiceError::StartFailed)?;

        let call_id = LiveVoiceCallId::new();
        reservation.activate(LiveVoiceCall::new(call_id.clone(), provider_connection))?;
        Ok(StartLiveVoiceCallResult { call_id, answer })
    }

    pub(super) async fn stop_call(
        &self,
        session_id: &str,
        call_id: &LiveVoiceCallId,
    ) -> Result<(), LiveVoiceError> {
        let call = self
            .calls_by_session
            .lock()
            .expect("live voice lock poisoned")
            .get(session_id)
            .and_then(|entry| match entry {
                LiveVoiceEntry::Starting => None,
                LiveVoiceEntry::Active(call) => Some(call.clone()),
            })
            .ok_or(LiveVoiceError::Unavailable)?;

        let final_state = {
            let mut call = call.lock().await;
            if call.id() != call_id {
                return Err(LiveVoiceError::Unavailable);
            }
            call.stop().await
        };

        if final_state.is_terminal() {
            self.remove_call(session_id, &call);
        }
        match final_state {
            LiveVoiceCallState::Stopped => Ok(()),
            LiveVoiceCallState::Failed => Err(LiveVoiceError::StopFailed),
            LiveVoiceCallState::Live | LiveVoiceCallState::Stopping => {
                Err(LiveVoiceError::StopFailed)
            }
        }
    }

    fn reserve(&self, session_id: &str) -> Result<StartReservation<'_>, LiveVoiceError> {
        let mut calls = self
            .calls_by_session
            .lock()
            .expect("live voice lock poisoned");
        if calls.contains_key(session_id) {
            return Err(LiveVoiceError::Unavailable);
        }
        calls.insert(session_id.to_string(), LiveVoiceEntry::Starting);
        Ok(StartReservation {
            calls_by_session: &self.calls_by_session,
            session_id: session_id.to_string(),
            activated: false,
        })
    }

    fn remove_call(&self, session_id: &str, call: &Arc<Mutex<LiveVoiceCall>>) {
        let mut calls = self
            .calls_by_session
            .lock()
            .expect("live voice lock poisoned");
        if matches!(
            calls.get(session_id),
            Some(LiveVoiceEntry::Active(current)) if Arc::ptr_eq(current, call)
        ) {
            calls.remove(session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goose_providers::live_voice_provider::fake::{
        provider_channel, provider_channel_with_availability,
    };
    use tokio::task::JoinHandle;

    type StartResult = Result<StartLiveVoiceCallResult, LiveVoiceError>;

    fn service(availability: LiveVoiceProviderAvailability) -> LiveVoiceService {
        let (provider, _starts) = provider_channel_with_availability(availability);
        LiveVoiceService::new(provider)
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
                    false,
                    WebRtcOffer::new(offer.into()).unwrap(),
                )
                .await
        })
    }

    fn assert_availability(service: &LiveVoiceService, expected: LiveVoiceAvailability) {
        assert_eq!(
            service.availability("main-session", GooseMode::Auto, false),
            expected
        );
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
        assert_eq!(
            ready.availability("main-session", GooseMode::Auto, true),
            LiveVoiceAvailability::ChatBusy
        );
        assert_eq!(
            ready.availability("main-session", GooseMode::Approve, false),
            LiveVoiceAvailability::RequiresAutonomousMode
        );
        assert_availability(&ready, LiveVoiceAvailability::Ready);
    }

    #[tokio::test]
    async fn a_start_reserves_the_session_until_it_finishes() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(provider));
        let first = spawn_start(service.clone(), "main-session", "first-offer");
        let pending = starts.recv().await.unwrap();

        assert_availability(&service, LiveVoiceAvailability::ChatBusy);

        let second = service
            .start_call(
                "main-session",
                GooseMode::Auto,
                false,
                WebRtcOffer::new("second-offer".into()).unwrap(),
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
        let service = Arc::new(LiveVoiceService::new(provider));
        let start_task = spawn_start(service.clone(), "main-session", "offer");
        let pending = starts.recv().await.unwrap();

        start_task.abort();
        assert!(matches!(start_task.await, Err(error) if error.is_cancelled()));
        assert_availability(&service, LiveVoiceAvailability::Ready);
        drop(pending);
    }

    #[tokio::test]
    async fn stop_failure_releases_the_session() {
        let (provider, mut starts) = provider_channel();
        let service = Arc::new(LiveVoiceService::new(provider));
        let start_task = spawn_start(service.clone(), "main-session", "offer");
        let mut connection = starts
            .recv()
            .await
            .unwrap()
            .accept(WebRtcAnswer::new("answer".into()).unwrap())
            .unwrap();
        let started = start_task.await.unwrap().unwrap();

        let stop_service = service.clone();
        let stop = tokio::spawn(async move {
            stop_service
                .stop_call("main-session", &started.call_id)
                .await
        });
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
}
