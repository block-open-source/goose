//! Provider-independent lifecycle for one Live voice connection.
//!
//! The coordinator reserves an owner/session generation before provider startup.
//! `startup` owns provider creation, `lifecycle` owns the authoritative event stream,
//! and `cleanup` releases only that exact generation after bounded teardown.

mod cleanup;
mod lifecycle;
mod startup;
mod types;
mod update_sink;

pub use types::*;

use goose_providers::live_voice_provider::LiveVoiceProvider;
use lifecycle::{ConnectionKey, LifecycleAction};
use startup::{run, ConnectionContext};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Clone)]
pub struct LiveVoiceCoordinator {
    inner: Arc<CoordinatorInner>,
}

pub(super) struct CoordinatorInner {
    provider: Arc<dyn LiveVoiceProvider>,
    deadlines: Deadlines,
    registry: Mutex<ConnectionRegistry>,
}

#[derive(Default)]
struct ConnectionRegistry {
    highest_attempt: HashMap<ConnectionKey, StartAttempt>,
    current: HashMap<ConnectionKey, CurrentConnection>,
}

struct CurrentConnection {
    attempt: StartAttempt,
    id: LiveConnectionId,
    actions: mpsc::UnboundedSender<LifecycleAction>,
}

#[derive(Clone, Copy)]
struct Deadlines {
    provider_start: Duration,
    readiness: Duration,
    acknowledgement: Duration,
    provider_close: Duration,
    cleanup: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            provider_start: Duration::from_secs(20),
            readiness: Duration::from_secs(15),
            acknowledgement: Duration::from_secs(8),
            provider_close: Duration::from_secs(12),
            cleanup: Duration::from_secs(18),
        }
    }
}

impl LiveVoiceCoordinator {
    pub fn new(provider: Arc<dyn LiveVoiceProvider>) -> Self {
        Self::with_deadlines(provider, Deadlines::default())
    }

    fn with_deadlines(provider: Arc<dyn LiveVoiceProvider>, deadlines: Deadlines) -> Self {
        Self {
            inner: Arc::new(CoordinatorInner {
                provider,
                deadlines,
                registry: Mutex::new(ConnectionRegistry::default()),
            }),
        }
    }

    pub async fn start(&self, request: StartRequest) -> Result<StartResponse, CoordinatorError> {
        let key = ConnectionKey::new(request.owner.clone(), request.live_session_id.clone());
        let id = LiveConnectionId(format!("live_{}", Uuid::now_v7()));
        let (actions, action_rx) = mpsc::unbounded_channel();
        {
            // The only coordinator lock. It is never held across provider or network awaits.
            let mut registry = self
                .inner
                .registry
                .lock()
                .expect("live voice lock poisoned");
            let highest = registry.highest_attempt.get(&key).copied().unwrap_or(0);
            if request.attempt <= highest {
                return Err(CoordinatorError::StaleAttempt);
            }
            registry
                .highest_attempt
                .insert(key.clone(), request.attempt);
            if registry.current.contains_key(&key) {
                return Err(CoordinatorError::Busy);
            }
            registry.current.insert(
                key.clone(),
                CurrentConnection {
                    attempt: request.attempt,
                    id: id.clone(),
                    actions,
                },
            );
        }

        let (reply, response) = oneshot::channel();
        tokio::spawn(run(ConnectionContext {
            inner: self.inner.clone(),
            key,
            id,
            request,
            actions: action_rx,
            reply: Some(reply),
        }));
        response.await.unwrap_or(Err(CoordinatorError::Cancelled))
    }

    pub fn command(
        &self,
        owner: &LiveOwnerToken,
        live_session_id: &LiveSessionId,
        live_connection_id: &LiveConnectionId,
        command: LiveVoiceCommand,
    ) -> Result<(), CoordinatorError> {
        let registry = self
            .inner
            .registry
            .lock()
            .expect("live voice lock poisoned");
        let current = registry
            .current
            .get(&ConnectionKey::new(owner.clone(), live_session_id.clone()))
            .filter(|current| &current.id == live_connection_id)
            .ok_or(CoordinatorError::StaleConnection)?;
        current
            .actions
            .send(LifecycleAction::Command(command))
            .map_err(|_| CoordinatorError::StaleConnection)
    }

    pub fn stop(
        &self,
        owner: &LiveOwnerToken,
        live_session_id: &LiveSessionId,
        target: StopTarget,
    ) -> Result<(), CoordinatorError> {
        let key = ConnectionKey::new(owner.clone(), live_session_id.clone());
        let mut registry = self
            .inner
            .registry
            .lock()
            .expect("live voice lock poisoned");
        match target {
            StopTarget::Attempt(attempt) => {
                let highest = registry.highest_attempt.entry(key.clone()).or_default();
                *highest = (*highest).max(attempt);
                if let Some(current) = registry
                    .current
                    .get(&key)
                    .filter(|current| current.attempt == attempt)
                {
                    let _ = current
                        .actions
                        .send(LifecycleAction::Stop(LiveTerminalReason::Cancelled));
                }
            }
            StopTarget::Connection(id) => {
                let current = registry
                    .current
                    .get(&key)
                    .filter(|current| current.id == id)
                    .ok_or(CoordinatorError::StaleConnection)?;
                let _ = current
                    .actions
                    .send(LifecycleAction::Stop(LiveTerminalReason::Stopped));
            }
        }
        Ok(())
    }

    pub fn owner_lost(&self, owner: &LiveOwnerToken) {
        let mut registry = self
            .inner
            .registry
            .lock()
            .expect("live voice lock poisoned");
        registry
            .highest_attempt
            .retain(|key, _| &key.owner != owner);
        for (key, current) in &registry.current {
            if &key.owner == owner {
                let _ = current
                    .actions
                    .send(LifecycleAction::Stop(LiveTerminalReason::OwnerLost));
            }
        }
    }

    pub fn live_session_closed(&self, owner: &LiveOwnerToken, session: &LiveSessionId) {
        let registry = self
            .inner
            .registry
            .lock()
            .expect("live voice lock poisoned");
        if let Some(current) = registry
            .current
            .get(&ConnectionKey::new(owner.clone(), session.clone()))
        {
            let _ = current
                .actions
                .send(LifecycleAction::Stop(LiveTerminalReason::SessionClosed));
        }
    }

    #[cfg(test)]
    fn has_connection(&self, owner: &LiveOwnerToken, session: &LiveSessionId) -> bool {
        self.inner
            .registry
            .lock()
            .expect("live voice lock poisoned")
            .current
            .contains_key(&ConnectionKey::new(owner.clone(), session.clone()))
    }
}

fn release(inner: &CoordinatorInner, key: &ConnectionKey, id: &LiveConnectionId) {
    let mut registry = inner.registry.lock().expect("live voice lock poisoned");
    if registry
        .current
        .get(key)
        .is_some_and(|current| &current.id == id)
    {
        registry.current.remove(key);
    }
}

#[cfg(test)]
mod tests;
