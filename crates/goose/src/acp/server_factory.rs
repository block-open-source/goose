use crate::acp::server::{
    AcpBuiltinSelection, AcpProviderFactory, ActiveRunRegistry, GooseAcpAgent, GooseAcpAgentOptions,
};
use crate::agents::GoosePlatform;
use crate::scheduler_trait::SchedulerTrait;
use crate::session::SessionManager;
use crate::source_roots::SourceRoot;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::info;

pub struct AcpServerFactoryConfig {
    pub builtins: AcpBuiltinSelection,
    pub data_dir: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    pub goose_platform: GoosePlatform,
    pub additional_source_roots: Vec<SourceRoot>,
    /// When set, new sessions use this host-controlled working directory
    /// instead of the `cwd` the connecting client sends. Used by roaming, where
    /// the connector's absolute path is meaningless on the host machine.
    pub session_cwd: Option<std::path::PathBuf>,
    pub enable_scheduler: bool,
}

pub struct AcpServer {
    config: AcpServerFactoryConfig,
    scheduler: OnceCell<Arc<dyn SchedulerTrait>>,
    active_prompt_runs: ActiveRunRegistry,
}

impl AcpServer {
    pub fn new(config: AcpServerFactoryConfig) -> Self {
        Self {
            config,
            scheduler: OnceCell::new(),
            active_prompt_runs: ActiveRunRegistry::default(),
        }
    }

    /// Start the scheduler now instead of on first client connect, so a
    /// headless `goose serve` runs scheduled jobs; on failure `create_agent`
    /// retries. No-op when the scheduler is disabled.
    pub async fn start_scheduler(&self) -> Result<()> {
        self.scheduler().await.map(|_| ())
    }

    async fn scheduler(&self) -> Result<Option<Arc<dyn SchedulerTrait>>> {
        if !self.config.enable_scheduler {
            return Ok(None);
        }

        let data_dir = self.config.data_dir.clone();
        self.scheduler
            .get_or_try_init(|| async move {
                let session_manager = Arc::new(SessionManager::new(data_dir.clone()));
                let schedule_file_path = data_dir.join("schedule.json");
                let scheduler =
                    crate::scheduler::Scheduler::new(schedule_file_path, session_manager)
                        .await
                        .map(|scheduler| scheduler as Arc<dyn SchedulerTrait>)?;
                Ok(scheduler)
            })
            .await
            .cloned()
            .map(Some)
    }

    pub async fn create_agent(&self) -> Result<Arc<GooseAcpAgent>> {
        self.create_agent_with_session_cwd(self.config.session_cwd.clone())
            .await
    }

    /// Create an agent whose sessions use `session_cwd` instead of this
    /// server's configured default. Used by the roaming bridge on `goose
    /// serve --roam`: the serve-wide server keeps `session_cwd: None` for
    /// local ACP clients whose paths are real on this machine, while each
    /// roaming connection gets a host-controlled working directory (the
    /// connector's absolute path is meaningless here). The agent still shares
    /// this server's active-run registry.
    pub async fn create_agent_with_session_cwd(
        &self,
        session_cwd: Option<std::path::PathBuf>,
    ) -> Result<Arc<GooseAcpAgent>> {
        let config = crate::config::Config::global();
        let disable_session_naming = config.get_goose_disable_session_naming().unwrap_or(false);
        let scheduler = self.scheduler().await?;
        if let Some(scheduler) = &scheduler {
            // Listing syncs from storage, registering jobs persisted by other processes.
            scheduler.list_scheduled_jobs().await;
        }

        let provider_factory: AcpProviderFactory = Arc::new(
            move |provider_name, extensions, working_dir, use_default_model| {
                Box::pin(async move {
                    if use_default_model {
                        crate::providers::create_with_default_model(&provider_name, extensions)
                            .await
                    } else {
                        match working_dir {
                            Some(working_dir) => {
                                crate::providers::create_with_working_dir(
                                    &provider_name,
                                    extensions,
                                    working_dir,
                                )
                                .await
                            }
                            None => crate::providers::create(&provider_name, extensions).await,
                        }
                    }
                })
            },
        );

        let agent = GooseAcpAgent::new(GooseAcpAgentOptions {
            provider_factory,
            builtin_selection: self.config.builtins.clone(),
            data_dir: self.config.data_dir.clone(),
            config_dir: self.config.config_dir.clone(),
            disable_session_naming,
            goose_platform: self.config.goose_platform.clone(),
            additional_source_roots: self.config.additional_source_roots.clone(),
            session_cwd,
            scheduler,
            active_prompt_runs: self.active_prompt_runs.clone(),
        })
        .await?;
        info!("Created new ACP agent");

        Ok(Arc::new(agent))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(data_dir: std::path::PathBuf, enable_scheduler: bool) -> AcpServer {
        AcpServer::new(AcpServerFactoryConfig {
            builtins: AcpBuiltinSelection::default(),
            config_dir: data_dir.clone(),
            data_dir,
            goose_platform: GoosePlatform::GooseCli,
            additional_source_roots: Vec::new(),
            session_cwd: None,
            enable_scheduler,
        })
    }

    #[tokio::test]
    async fn disabled_server_does_not_construct_scheduler() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), false);

        assert!(server.scheduler().await.unwrap().is_none());
        assert!(!root.path().join("schedule.json").exists());
    }

    #[tokio::test]
    async fn automatic_server_constructs_scheduler() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), true);

        assert!(server.scheduler().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn agents_from_one_server_share_the_active_run_registry() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), false);

        let a = server.create_agent().await.unwrap();
        let b = server.create_agent().await.unwrap();

        assert!(
            Arc::ptr_eq(a.active_run_registry(), b.active_run_registry()),
            "each connection's agent must share one per-session run registry so \
             the active-run guard holds across roaming connections"
        );
    }

    #[tokio::test]
    async fn steer_routes_to_the_agent_that_owns_the_run() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), false);

        let running = server.create_agent().await.unwrap();
        let steering = server.create_agent().await.unwrap();

        let owner = Arc::new(crate::agents::Agent::new());
        running
            .test_start_active_run("session-1", "run-1".to_string(), owner.clone())
            .await
            .unwrap();

        let (run_id, resolved) = steering
            .test_require_active_run("session-1", "run-1")
            .await
            .unwrap();

        assert_eq!(run_id, "run-1");
        assert!(
            Arc::ptr_eq(&resolved, &owner),
            "a steer arriving on a second roaming connection must resolve the \
             agent running the prompt, not the caller's connection-local agent"
        );
    }

    #[tokio::test]
    async fn dropping_a_prompt_future_releases_the_shared_run() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), false);

        let running = server.create_agent().await.unwrap();
        let owner = Arc::new(crate::agents::Agent::new());
        running
            .test_start_active_run("session-1", "run-1".to_string(), owner)
            .await
            .unwrap();

        running.test_drop_active_run_guard("session-1", "run-1");
        tokio::task::yield_now().await;

        let second = server.create_agent().await.unwrap();
        assert!(
            second
                .test_require_active_run("session-1", "run-1")
                .await
                .is_err(),
            "a dropped prompt future must release its run so later \
             connections are not permanently locked out of the session"
        );
    }

    #[tokio::test]
    async fn start_scheduler_initializes_before_any_client_connects() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), true);

        assert!(!server.scheduler.initialized());
        server.start_scheduler().await.unwrap();
        assert!(server.scheduler.initialized());
    }

    #[tokio::test]
    async fn start_scheduler_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), true);

        server.start_scheduler().await.unwrap();
        server.start_scheduler().await.unwrap();
    }

    #[tokio::test]
    async fn start_scheduler_does_not_construct_one_when_disabled() {
        let root = tempfile::tempdir().unwrap();
        let server = server(root.path().to_path_buf(), false);

        server.start_scheduler().await.unwrap();
        assert!(!server.scheduler.initialized());
    }
}
