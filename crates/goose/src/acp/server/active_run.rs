use crate::agents::Agent;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct ActiveRun {
    run_id: String,
    cancel_token: CancellationToken,
    /// Routes steering from another roaming connection to the run owner.
    agent: Arc<Agent>,
}

struct SessionRunState {
    agent_run: Option<ActiveRun>,
    live_active: bool,
}

pub(super) enum StartRunError {
    AgentRunExists { run_id: String },
    LiveCallExists,
    LiveCallMissing,
}

#[derive(Default)]
pub struct ActiveRunRegistry {
    runs_by_session: Mutex<HashMap<String, SessionRunState>>,
}

impl ActiveRunRegistry {
    pub(super) fn start_prompt_run(
        &self,
        session_id: &str,
        run_id: String,
        cancel_token: CancellationToken,
        agent: Arc<Agent>,
    ) -> Result<(), StartRunError> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if let Some(state) = runs.get(session_id) {
            if let Some(agent_run) = &state.agent_run {
                return Err(StartRunError::AgentRunExists {
                    run_id: agent_run.run_id.clone(),
                });
            }
            if state.live_active {
                return Err(StartRunError::LiveCallExists);
            }
        }
        runs.insert(
            session_id.to_string(),
            SessionRunState {
                agent_run: Some(ActiveRun {
                    run_id,
                    cancel_token,
                    agent,
                }),
                live_active: false,
            },
        );
        Ok(())
    }

    pub(super) fn start_live_delegation(
        &self,
        session_id: &str,
        run_id: String,
        cancel_token: CancellationToken,
        agent: Arc<Agent>,
    ) -> Result<(), StartRunError> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        let Some(state) = runs.get_mut(session_id) else {
            return Err(StartRunError::LiveCallMissing);
        };
        if let Some(agent_run) = &state.agent_run {
            return Err(StartRunError::AgentRunExists {
                run_id: agent_run.run_id.clone(),
            });
        }
        state.agent_run = Some(ActiveRun {
            run_id,
            cancel_token,
            agent,
        });
        Ok(())
    }

    pub(super) fn agent_run(&self, session_id: &str) -> Option<(String, Arc<Agent>)> {
        self.runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
            .and_then(|state| state.agent_run.as_ref())
            .map(|run| (run.run_id.clone(), run.agent.clone()))
    }

    pub(super) fn agent_cancel_token(&self, session_id: &str) -> Option<CancellationToken> {
        self.runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
            .and_then(|state| state.agent_run.as_ref())
            .map(|run| run.cancel_token.clone())
    }

    pub(super) fn remove_agent_run(&self, session_id: &str, run_id: &str) -> Option<Arc<Agent>> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        let state = runs.get_mut(session_id)?;
        if state.agent_run.as_ref()?.run_id != run_id {
            return None;
        }
        let agent = state.agent_run.take()?.agent;
        if !state.live_active {
            runs.remove(session_id);
        }
        Some(agent)
    }

    pub(super) fn start_live(&self, session_id: &str) -> bool {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if runs.contains_key(session_id) {
            return false;
        }
        runs.insert(
            session_id.to_string(),
            SessionRunState {
                agent_run: None,
                live_active: true,
            },
        );
        true
    }

    pub(super) fn finish_live(&self, session_id: &str) {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if let Some(state) = runs.get_mut(session_id) {
            state.live_active = false;
            if state.agent_run.is_none() {
                runs.remove(session_id);
            }
        }
    }

    pub(super) fn is_active(&self, session_id: &str) -> bool {
        self.runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_runs_are_scoped_by_session() {
        let registry = ActiveRunRegistry::default();

        assert!(registry.start_live("one"));
        assert!(!registry.start_live("one"));
        assert!(registry.start_live("two"));

        registry.finish_live("one");
        assert!(registry.start_live("one"));
    }

    #[tokio::test]
    async fn prompt_and_live_runs_conflict() {
        let registry = ActiveRunRegistry::default();

        assert!(registry
            .start_prompt_run(
                "session",
                "run".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            )
            .is_ok());
        assert!(!registry.start_live("session"));

        registry.remove_agent_run("session", "run");
        assert!(registry.start_live("session"));
        assert!(matches!(
            registry.start_prompt_run(
                "session",
                "run".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            ),
            Err(StartRunError::LiveCallExists)
        ));
    }

    #[tokio::test]
    async fn live_delegation_shares_the_live_session_without_admitting_another_prompt() {
        let registry = ActiveRunRegistry::default();
        assert!(registry.start_live("session"));
        assert!(registry
            .start_live_delegation(
                "session",
                "delegated".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            )
            .is_ok());
        assert!(matches!(
            registry.start_prompt_run(
                "session",
                "prompt".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            ),
            Err(StartRunError::AgentRunExists { .. })
        ));

        registry.finish_live("session");
        assert_eq!(
            registry.agent_run("session").map(|(run_id, _)| run_id),
            Some("delegated".into())
        );
    }
}
