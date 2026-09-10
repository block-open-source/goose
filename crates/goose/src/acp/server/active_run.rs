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

#[derive(Default)]
pub struct ActiveRunRegistry {
    runs_by_session: Mutex<HashMap<String, ActiveRun>>,
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
            return Err(active.clone());
        }
        runs.insert(
            session_id.to_string(),
            ActiveRun::Normal {
                run_id,
                cancel_token,
                agent,
            },
        );
        Ok(())
    }

    pub(super) fn normal_run(&self, session_id: &str) -> Option<(String, Arc<Agent>)> {
        match self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned")
            .get(session_id)
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
        let agent = match runs.get(session_id) {
            Some(ActiveRun::Normal {
                run_id: active_run_id,
                agent,
                ..
            }) if active_run_id == run_id => agent.clone(),
            _ => return None,
        };
        runs.remove(session_id);
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
        runs.insert(session_id.to_string(), ActiveRun::Live);
        true
    }

    pub(super) fn finish_live(&self, session_id: &str) {
        let mut runs = self
            .runs_by_session
            .lock()
            .expect("active run lock poisoned");
        if matches!(runs.get(session_id), Some(ActiveRun::Live)) {
            runs.remove(session_id);
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

    #[test]
    fn normal_and_live_runs_conflict() {
        let registry = ActiveRunRegistry::default();

        registry
            .start_normal(
                "session",
                "run".into(),
                CancellationToken::new(),
                Arc::new(Agent::new()),
            )
            .unwrap();
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
}
