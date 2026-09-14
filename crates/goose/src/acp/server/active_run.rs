use crate::agents::Agent;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) enum ActiveRun {
    Normal {
        run_id: String,
        cancel_token: CancellationToken,
        /// Routes steering from another roaming connection to the run owner.
        agent: Arc<Agent>,
    },
    Live,
}

#[derive(Clone, Default)]
struct SessionRuns {
    normal: Option<ActiveRun>,
    live: bool,
}

#[derive(Default)]
pub struct ActiveRunRegistry {
    runs_by_session: Mutex<HashMap<String, SessionRuns>>,
}

impl ActiveRunRegistry {
    pub(super) fn start_normal(
        &self,
        session_id: &str,
        run_id: String,
        cancel_token: CancellationToken,
        agent: Arc<Agent>,
    ) -> Result<(), ActiveRun> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if let Some(active) = runs.get(session_id) {
            if let Some(normal) = &active.normal {
                return Err(normal.clone());
            }
            if active.live {
                return Err(ActiveRun::Live);
            }
        }
        runs.entry(session_id.to_string()).or_default().normal = Some(ActiveRun::Normal {
            run_id,
            cancel_token,
            agent,
        });
        Ok(())
    }

    pub(super) fn start_live_delegation(
        &self,
        session_id: &str,
        run_id: String,
        cancel_token: CancellationToken,
        agent: Arc<Agent>,
    ) -> Result<(), ActiveRun> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        let active = runs.entry(session_id.to_string()).or_default();
        if let Some(normal) = &active.normal {
            return Err(normal.clone());
        }
        if !active.live {
            return Err(ActiveRun::Live);
        }
        active.normal = Some(ActiveRun::Normal {
            run_id,
            cancel_token,
            agent,
        });
        Ok(())
    }

    pub(super) fn normal_run(&self, session_id: &str) -> Option<(String, Arc<Agent>)> {
        match self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
            .and_then(|active| active.normal.as_ref())
        {
            Some(ActiveRun::Normal { run_id, agent, .. }) => Some((run_id.clone(), agent.clone())),
            _ => None,
        }
    }

    pub(super) fn normal_cancel_token(&self, session_id: &str) -> Option<CancellationToken> {
        match self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
            .and_then(|active| active.normal.as_ref())
        {
            Some(ActiveRun::Normal { cancel_token, .. }) => Some(cancel_token.clone()),
            _ => None,
        }
    }

    pub(super) fn remove_normal(&self, session_id: &str, run_id: &str) -> Option<Arc<Agent>> {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        let agent = match runs
            .get(session_id)
            .and_then(|active| active.normal.as_ref())
        {
            Some(ActiveRun::Normal {
                run_id: active_run_id,
                agent,
                ..
            }) if active_run_id == run_id => agent.clone(),
            _ => return None,
        };
        let active = runs.get_mut(session_id).expect("active run exists");
        active.normal = None;
        if !active.live {
            runs.remove(session_id);
        }
        Some(agent)
    }

    pub(super) fn start_live(&self, session_id: &str) -> bool {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if runs
            .get(session_id)
            .is_some_and(|active| active.live || active.normal.is_some())
        {
            return false;
        }
        runs.entry(session_id.to_string()).or_default().live = true;
        true
    }

    pub(super) fn finish_live(&self, session_id: &str) {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if let Some(active) = runs.get_mut(session_id) {
            active.live = false;
            if active.normal.is_none() {
                runs.remove(session_id);
            }
        }
    }

    pub(super) fn is_active(&self, session_id: &str) -> bool {
        self.runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
            .is_some_and(|active| active.live || active.normal.is_some())
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
    async fn normal_and_live_runs_conflict() {
        let registry = ActiveRunRegistry::default();

        assert!(registry
            .start_normal(
                "session",
                "run".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            )
            .is_ok());
        assert!(!registry.start_live("session"));

        registry.remove_normal("session", "run");
        assert!(registry.start_live("session"));
        assert!(matches!(
            registry.start_normal(
                "session",
                "run".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            ),
            Err(ActiveRun::Live)
        ));
    }

    #[tokio::test]
    async fn live_delegation_shares_the_live_session_without_admitting_a_normal_prompt() {
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
            registry.start_normal(
                "session",
                "normal".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            ),
            Err(ActiveRun::Normal { .. })
        ));

        registry.finish_live("session");
        assert_eq!(
            registry.normal_run("session").map(|(run_id, _)| run_id),
            Some("delegated".into())
        );
    }
}
