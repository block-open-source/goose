use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{anyhow, Result};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

use crate::permission::Permission;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConfirmationAnswer {
    LiveHandled,
    StateMachine(Permission),
}

pub(super) struct SessionToolConfirmationState {
    // Held for the lifetime of one Agent::reply stream, including confirmation waits and resumes.
    turn_lock: Arc<Mutex<()>>,
    // Serializes submit_tool_confirmation so concurrent answers cannot both be accepted.
    pub(super) confirmation_submission_lock: Mutex<()>,
    // Tracks requests from the current confirmation pause; None means still unanswered.
    confirmations: StdMutex<HashMap<String, Option<ConfirmationAnswer>>>,
    // Wakes wait_for_all_confirmation_answers; confirmations remains the source of truth.
    confirmation_answered: Notify,
}

impl SessionToolConfirmationState {
    fn new() -> Self {
        Self {
            turn_lock: Arc::new(Mutex::new(())),
            confirmation_submission_lock: Mutex::new(()),
            confirmations: StdMutex::new(HashMap::new()),
            confirmation_answered: Notify::new(),
        }
    }

    pub(super) fn try_start_turn(self: &Arc<Self>) -> Result<ActiveTurnGuard> {
        let turn_lock_guard = self
            .turn_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| anyhow!("session already has an active turn"))?;
        Ok(ActiveTurnGuard {
            state: self.clone(),
            _turn_lock_guard: turn_lock_guard,
        })
    }

    pub(super) fn register_request(&self, request_id: String) {
        self.confirmations
            .lock()
            .expect("tool confirmation state unavailable")
            .entry(request_id)
            .or_insert(None);
    }

    pub(super) fn answer(&self, request_id: &str) -> Option<ConfirmationAnswer> {
        self.confirmations
            .lock()
            .expect("tool confirmation state unavailable")
            .get(request_id)
            .and_then(Clone::clone)
    }

    pub(super) fn contains_request(&self, request_id: &str) -> bool {
        self.confirmations
            .lock()
            .expect("tool confirmation state unavailable")
            .contains_key(request_id)
    }

    pub(super) fn record_answer(&self, request_id: &str, answer: ConfirmationAnswer) -> Result<()> {
        let mut confirmations = self
            .confirmations
            .lock()
            .expect("tool confirmation state unavailable");
        match confirmations.get_mut(request_id) {
            None => return Err(anyhow!("tool confirmation request is no longer active")),
            Some(Some(_)) => return Err(anyhow!("tool confirmation request was already answered")),
            Some(slot @ None) => *slot = Some(answer),
        }
        drop(confirmations);
        self.confirmation_answered.notify_waiters();
        Ok(())
    }

    pub(super) async fn wait_for_all_confirmation_answers(
        &self,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        loop {
            let answer_received = self.confirmation_answered.notified();
            tokio::pin!(answer_received);
            answer_received.as_mut().enable();

            let completed = {
                let confirmations = self
                    .confirmations
                    .lock()
                    .expect("tool confirmation state unavailable");
                (!confirmations.is_empty() && confirmations.values().all(Option::is_some)).then(
                    || {
                        confirmations.values().any(|answer| {
                            matches!(answer, Some(ConfirmationAnswer::StateMachine(_)))
                        })
                    },
                )
            };
            if let Some(has_state_machine_answer) = completed {
                return Ok(has_state_machine_answer);
            }

            tokio::select! {
                _ = answer_received => {}
                _ = cancel.cancelled() => return Err(anyhow!("state-machine turn cancelled")),
            }
        }
    }

    pub(super) fn clear_confirmations(&self) {
        self.confirmations
            .lock()
            .expect("tool confirmation state unavailable")
            .clear();
    }
}

pub(super) struct ActiveTurnGuard {
    state: Arc<SessionToolConfirmationState>,
    _turn_lock_guard: OwnedMutexGuard<()>,
}

impl ActiveTurnGuard {
    pub(super) fn state(&self) -> &Arc<SessionToolConfirmationState> {
        &self.state
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        self.state.clear_confirmations();
    }
}

pub(super) struct ToolConfirmationCoordinator {
    sessions: StdMutex<HashMap<String, Arc<SessionToolConfirmationState>>>,
}

impl ToolConfirmationCoordinator {
    pub(super) fn new() -> Self {
        Self {
            sessions: StdMutex::new(HashMap::new()),
        }
    }

    pub(super) fn session(&self, session_id: &str) -> Arc<SessionToolConfirmationState> {
        self.sessions
            .lock()
            .expect("tool confirmation coordinator unavailable")
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(SessionToolConfirmationState::new()))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_active_turn_and_releases_on_drop() {
        let coordinator = ToolConfirmationCoordinator::new();
        let session = coordinator.session("session");
        let guard = session.try_start_turn().unwrap();

        assert!(session.try_start_turn().is_err());

        drop(guard);
        assert!(session.try_start_turn().is_ok());
    }

    #[tokio::test]
    async fn waits_for_every_confirmation_in_the_batch() {
        let coordinator = ToolConfirmationCoordinator::new();
        let session = coordinator.session("session");
        let _guard = session.try_start_turn().unwrap();
        session.register_request("request-1".to_string());
        session.register_request("request-2".to_string());
        session
            .record_answer(
                "request-1",
                ConfirmationAnswer::StateMachine(Permission::AllowOnce),
            )
            .unwrap();
        session
            .record_answer("request-2", ConfirmationAnswer::LiveHandled)
            .unwrap();

        let has_state_machine_answer = session
            .wait_for_all_confirmation_answers(&CancellationToken::new())
            .await
            .unwrap();

        assert!(has_state_machine_answer);
    }

    #[test]
    fn active_turn_drop_clears_pending_requests() {
        let coordinator = ToolConfirmationCoordinator::new();
        let session = coordinator.session("session");
        let guard = session.try_start_turn().unwrap();
        session.register_request("request".to_string());
        assert!(session.contains_request("request"));

        drop(guard);

        assert!(!session.contains_request("request"));
        assert!(session.answer("request").is_none());
    }

    #[test]
    fn first_answer_is_immutable() {
        let coordinator = ToolConfirmationCoordinator::new();
        let session = coordinator.session("session");
        let _guard = session.try_start_turn().unwrap();
        session.register_request("request".to_string());
        session
            .record_answer(
                "request",
                ConfirmationAnswer::StateMachine(Permission::AllowOnce),
            )
            .unwrap();

        assert!(session
            .record_answer(
                "request",
                ConfirmationAnswer::StateMachine(Permission::DenyOnce),
            )
            .is_err());
        assert_eq!(
            session.answer("request"),
            Some(ConfirmationAnswer::StateMachine(Permission::AllowOnce))
        );
    }
}
