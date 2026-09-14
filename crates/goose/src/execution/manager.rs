use crate::agents::mcp_client::GooseMcpHostInfo;
use crate::agents::{Agent, AgentConfig, ExtensionLoadResult, GoosePlatform};
use crate::config::permission::PermissionManager;
use crate::config::Config;
use crate::scheduler_trait::SchedulerTrait;
use crate::session::{SessionManager, SessionNameUpdate};
use anyhow::Result;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, OnceCell, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

const DEFAULT_MAX_SESSION: usize = 100;

static AGENT_MANAGER: OnceCell<Arc<AgentManager>> = OnceCell::const_new();

#[derive(Clone, Default)]
pub struct RuntimeContext {
    pub mcp_host_info: Option<GooseMcpHostInfo>,
    pub use_login_shell_path: Option<bool>,
    pub session_name_update_tx: Option<mpsc::UnboundedSender<SessionNameUpdate>>,
}

pub struct AgentManagerGetResult {
    pub agent: Arc<Agent>,
    pub agent_created: bool,
    pub extension_results: Vec<ExtensionLoadResult>,
}

pub struct AgentManager {
    sessions: Arc<RwLock<LruCache<String, Arc<Agent>>>>,
    agent_config: AgentConfig,
    default_provider: Arc<RwLock<Option<Arc<dyn crate::providers::base::Provider>>>>,
    cancel_tokens: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Per-session creation locks.  When `get_or_create_agent` misses the
    /// `sessions` cache it acquires the per-session lock before doing the
    /// expensive work (provider restore, MCP extension initialization) so
    /// concurrent callers for the same session never race into doing the
    /// work twice.  Entries are inserted on demand and pruned when the
    /// session is removed *or* evicted by the LRU; the underlying
    /// `Arc<Mutex<()>>` stays alive as long as any caller still holds it,
    /// even after the HashMap entry is removed.
    creation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl AgentManager {
    pub async fn new(agent_config: AgentConfig, max_sessions: Option<usize>) -> Result<Self> {
        let capacity = NonZeroUsize::new(max_sessions.unwrap_or(DEFAULT_MAX_SESSION))
            .unwrap_or_else(|| NonZeroUsize::new(100).unwrap());

        let manager = Self {
            sessions: Arc::new(RwLock::new(LruCache::new(capacity))),
            agent_config,
            default_provider: Arc::new(RwLock::new(None)),
            cancel_tokens: Arc::new(RwLock::new(HashMap::new())),
            creation_locks: Arc::new(Mutex::new(HashMap::new())),
        };

        Ok(manager)
    }

    pub async fn instance() -> Result<Arc<Self>> {
        AGENT_MANAGER
            .get_or_try_init(|| async {
                let config = Config::global();
                let max_sessions = config
                    .get_goose_max_active_agents()
                    .unwrap_or(DEFAULT_MAX_SESSION);
                let default_mode = config.get_goose_mode().unwrap_or_default();
                let session_manager = Arc::new(SessionManager::instance());
                let agent_config = AgentConfig::new(
                    session_manager,
                    PermissionManager::instance(),
                    None,
                    default_mode,
                    config.get_goose_disable_session_naming().unwrap_or(false),
                    GoosePlatform::GooseDesktop,
                );
                let manager = Self::new(agent_config, Some(max_sessions)).await?;
                Ok(Arc::new(manager))
            })
            .await
            .cloned()
    }

    pub fn scheduler(&self) -> Option<Arc<dyn SchedulerTrait>> {
        self.agent_config.scheduler_service.as_ref().map(Arc::clone)
    }

    /// Get the shared SessionManager for session-only operations
    pub fn session_manager(&self) -> &SessionManager {
        self.agent_config.session_manager.as_ref()
    }

    pub async fn set_default_provider(&self, provider: Arc<dyn crate::providers::base::Provider>) {
        debug!("Setting default provider on AgentManager");
        *self.default_provider.write().await = Some(provider);
    }

    pub async fn get_or_create_agent(&self, session_id: String) -> Result<Arc<Agent>> {
        Ok(self
            .get_or_create_agent_with_runtime_context(session_id, RuntimeContext::default())
            .await?
            .agent)
    }

    pub async fn get_or_create_agent_with_runtime_context(
        &self,
        session_id: String,
        runtime_context: RuntimeContext,
    ) -> Result<AgentManagerGetResult> {
        // Fast path: agent already cached.
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get(&session_id) {
                return Ok(AgentManagerGetResult {
                    agent: Arc::clone(existing),
                    agent_created: false,
                    extension_results: Vec::new(),
                });
            }
        }

        // Slow path: serialize creation per session so concurrent callers
        // (e.g. start_agent's background extension-loading task and a
        // resume_agent request racing through the frontend) cannot each
        // construct their own Agent and independently send `initialize` to
        // every MCP server.  See issue #9031.
        let creation_lock = {
            let mut locks = self.creation_locks.lock().await;
            Arc::clone(
                locks
                    .entry(session_id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let creation_guard = creation_lock.lock().await;

        // Funnel the fallible work through a helper so we can prune the
        // per-session creation lock on every error exit.  Without this
        // the provider-setup path (update_provider / update_mode) could
        // bail out via `?`, leaving a permanent `creation_locks` entry
        // for a session that never made it into the LRU cache and that
        // no one will ever call `remove_session` on.
        let result = self.create_agent_locked(&session_id, runtime_context).await;

        if result.is_err() {
            // Release BOTH the guard and our local Arc clone of the
            // creation lock before pruning.  `prune_creation_lock`
            // gates removal on `Arc::strong_count == 1`; if we kept
            // `creation_lock` alive the count would still be at least
            // two (HashMap + this local) and the failed session would
            // leak its lock entry forever.  In-flight waiters keep the
            // Arc alive on their own and prune correctly skips while
            // they hold it.
            drop(creation_guard);
            drop(creation_lock);
            self.prune_creation_lock(&session_id).await;
        }

        result
    }

    /// Slow-path body for `get_or_create_agent`.  Must be called with the
    /// per-session creation lock held by the caller.
    async fn create_agent_locked(
        &self,
        session_id: &str,
        runtime_context: RuntimeContext,
    ) -> Result<AgentManagerGetResult> {
        // Re-check under the creation lock: another caller may have
        // finished creating the agent while we were waiting.
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get(session_id) {
                return Ok(AgentManagerGetResult {
                    agent: Arc::clone(existing),
                    agent_created: false,
                    extension_results: Vec::new(),
                });
            }
        }

        let mut mode = self.agent_config.goose_mode;
        if let Ok(session) = self
            .agent_config
            .session_manager
            .get_session(session_id, false)
            .await
        {
            mode = session.goose_mode;
            info!(goose_mode = %mode, session_id = %session_id, "Session loaded");
        }

        let mut config = self.agent_config.clone();
        config.goose_mode = mode;
        config.mcp_host_info = runtime_context.mcp_host_info;
        config.use_login_shell_path = runtime_context.use_login_shell_path;
        config.session_name_update_tx = runtime_context.session_name_update_tx;
        let agent = Arc::new(Agent::with_config(config));
        let mut extension_results = Vec::new();
        let mut provider_restore_error = None;

        if let Ok(session) = self
            .agent_config
            .session_manager
            .get_session(session_id, false)
            .await
        {
            if session.provider_name.is_some() {
                info!(
                    "Restoring evicted session {} (provider: {:?})",
                    session_id, session.provider_name
                );
                if let Err(error) = agent.restore_provider_from_session(&session).await {
                    if crate::acp::is_auth_required(&error) {
                        return Err(error);
                    }
                    tracing::warn!(
                        "Failed to restore provider for session {}: {}",
                        session_id,
                        error
                    );
                    provider_restore_error = Some(error);
                }
            }
            extension_results = agent.load_extensions_from_session(&session).await;
            if let Some(recipe) = &session.recipe {
                agent
                    .apply_recipe_components(recipe.response.clone(), true)
                    .await?;
            }
        }

        if agent.provider().await.is_err() {
            if let Some(provider) = &*self.default_provider.read().await {
                let config = crate::config::Config::global();
                let model_config = config
                    .get_goose_provider()
                    .ok()
                    .zip(config.get_goose_model().ok())
                    .and_then(|(provider_name, model_name)| {
                        crate::model_config::model_config_from_user_config(
                            &provider_name,
                            &model_name,
                        )
                        .ok()
                    })
                    .unwrap_or_else(|| goose_providers::model::ModelConfig::new("unknown"));
                agent
                    .update_provider(Arc::clone(provider), model_config, session_id)
                    .await?;
                provider
                    .update_mode(session_id, mode)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to propagate mode to provider: {}", e))?;
            }
        }

        // A session whose record names a provider must not be cached with a
        // provider-less agent: the cache fast path would keep returning the
        // broken agent, so every subsequent load fails with "Provider not
        // set" until the process restarts — even after the underlying issue
        // (e.g. expired cloud credentials) is fixed.  Surface the restore
        // error instead so the next attempt runs the restore again.
        if agent.provider().await.is_err() {
            if let Some(error) = provider_restore_error {
                return Err(error);
            }
        }

        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get(session_id) {
            return Ok(AgentManagerGetResult {
                agent: Arc::clone(existing),
                agent_created: false,
                extension_results: Vec::new(),
            });
        }
        // `push` returns the LRU-evicted entry when the cache is at
        // capacity, which `put` does not surface.  We need the evicted
        // key so we can also drop its creation lock below, otherwise the
        // `creation_locks` HashMap would grow without bound in long-lived
        // processes that churn through many sessions.
        let evicted = sessions
            .push(session_id.to_string(), agent.clone())
            .map(|(k, _)| k);
        drop(sessions);

        if let Some(evicted_id) = evicted {
            self.prune_creation_lock(&evicted_id).await;
        }

        Ok(AgentManagerGetResult {
            agent,
            agent_created: true,
            extension_results,
        })
    }

    /// Drop the per-session creation lock for `session_id` if no other
    /// caller is currently holding a clone of its `Arc`.  Holding the
    /// `creation_locks` mutex while we both check `Arc::strong_count` and
    /// remove guarantees no new waiter can race in between the check and
    /// the removal: any new caller would need to acquire the outer mutex
    /// first to clone the inner `Arc`.
    ///
    /// If a waiter is still in flight (strong_count > 1) we leave the
    /// entry in place so the in-flight callers continue to serialize
    /// through the same lock; a later removal or eviction will sweep it.
    async fn prune_creation_lock(&self, session_id: &str) {
        let mut locks = self.creation_locks.lock().await;
        let in_use = locks
            .get(session_id)
            .is_some_and(|lock| Arc::strong_count(lock) > 1);
        if !in_use {
            locks.remove(session_id);
        }
    }

    pub async fn remove_session(&self, session_id: &str) -> Result<()> {
        if let Some(token) = self.cancel_tokens.write().await.remove(session_id) {
            token.cancel();
        }
        let mut sessions = self.sessions.write().await;
        sessions
            .pop(session_id)
            .ok_or_else(|| anyhow::anyhow!("Session {} not found", session_id))?;
        drop(sessions);
        // Best-effort prune of the per-session creation lock so the
        // HashMap doesn't grow unbounded.  Any caller still holding a
        // clone of the Arc keeps the underlying Mutex alive until it
        // releases its guard.
        self.prune_creation_lock(session_id).await;
        info!("Removed session {}", session_id);
        Ok(())
    }

    /// Drops an in-memory agent when one is loaded for `session_id`.
    pub async fn remove_session_if_loaded(&self, session_id: &str) -> Result<()> {
        if let Some(token) = self.cancel_tokens.write().await.remove(session_id) {
            token.cancel();
        }
        let mut sessions = self.sessions.write().await;
        if sessions.pop(session_id).is_none() {
            return Ok(());
        }
        drop(sessions);
        self.prune_creation_lock(session_id).await;
        info!("Removed session {}", session_id);
        Ok(())
    }

    pub async fn has_session(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains(session_id)
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Atomically check if busy and register a cancel token. Returns Err if already busy.
    pub async fn try_register_cancel_token(
        &self,
        session_id: &str,
        token: CancellationToken,
    ) -> Result<()> {
        let mut tokens = self.cancel_tokens.write().await;
        if tokens.contains_key(session_id) {
            anyhow::bail!("Session '{}' is currently busy", session_id);
        }
        tokens.insert(session_id.to_string(), token);
        Ok(())
    }

    /// Remove the cancellation token for a session (called when reply finishes)
    pub async fn unregister_cancel_token(&self, session_id: &str) {
        self.cancel_tokens.write().await.remove(session_id);
    }

    /// Cancel a running agent by triggering its cancellation token
    pub async fn cancel_session(&self, session_id: &str) -> Result<()> {
        let tokens = self.cancel_tokens.read().await;
        let token = tokens
            .get(session_id)
            .ok_or_else(|| anyhow::anyhow!("No active operation for session {}", session_id))?;
        token.cancel();
        Ok(())
    }

    /// Check if a session has an active reply in progress
    pub async fn is_session_busy(&self, session_id: &str) -> bool {
        let tokens = self.cancel_tokens.read().await;
        tokens.contains_key(session_id)
    }

    /// List session IDs that currently have active agents loaded
    pub async fn list_active_session_ids(&self) -> Vec<String> {
        self.sessions
            .read()
            .await
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tempfile::TempDir;

    use test_case::test_case;

    use crate::agents::{AgentConfig, GoosePlatform};
    use crate::config::permission::PermissionManager;
    use crate::config::GooseMode;
    use crate::execution::SessionExecutionMode;
    use crate::session::SessionManager;

    use super::AgentManager;

    async fn create_test_manager(temp_dir: &TempDir) -> AgentManager {
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let agent_config = AgentConfig::new(
            session_manager,
            PermissionManager::instance(),
            None,
            GooseMode::default(),
            false,
            GoosePlatform::GooseDesktop,
        );
        AgentManager::new(agent_config, Some(100)).await.unwrap()
    }

    #[test]
    fn test_execution_mode_constructors() {
        assert_eq!(
            SessionExecutionMode::chat(),
            SessionExecutionMode::Interactive
        );
        assert_eq!(
            SessionExecutionMode::scheduled(),
            SessionExecutionMode::Background
        );

        let parent = "parent-123".to_string();
        assert_eq!(
            SessionExecutionMode::task(parent.clone()),
            SessionExecutionMode::SubTask {
                parent_session: parent
            }
        );
    }

    #[tokio::test]
    async fn test_session_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;

        let session1 = uuid::Uuid::new_v4().to_string();
        let session2 = uuid::Uuid::new_v4().to_string();

        let agent1 = manager.get_or_create_agent(session1.clone()).await.unwrap();

        let agent2 = manager.get_or_create_agent(session2.clone()).await.unwrap();

        // Different sessions should have different agents
        assert!(!Arc::ptr_eq(&agent1, &agent2));

        // Getting the same session should return the same agent
        let agent1_again = manager.get_or_create_agent(session1).await.unwrap();

        assert!(Arc::ptr_eq(&agent1, &agent1_again));
    }

    #[tokio::test]
    async fn test_session_limit() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;

        let sessions: Vec<_> = (0..100).map(|i| format!("session-{}", i)).collect();

        for session in &sessions {
            manager.get_or_create_agent(session.clone()).await.unwrap();
        }

        // Create a new session after cleanup
        let new_session = "new-session".to_string();
        let _new_agent = manager.get_or_create_agent(new_session).await.unwrap();

        assert_eq!(manager.session_count().await, 100);
    }

    #[tokio::test]
    async fn test_remove_session() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        let session = String::from("remove-test");

        manager.get_or_create_agent(session.clone()).await.unwrap();
        assert!(manager.has_session(&session).await);

        manager.remove_session(&session).await.unwrap();
        assert!(!manager.has_session(&session).await);

        assert!(manager.remove_session(&session).await.is_err());
    }

    #[tokio::test]
    async fn test_remove_session_if_loaded() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        let session = String::from("remove-if-loaded-test");

        manager.remove_session_if_loaded(&session).await.unwrap();

        manager.get_or_create_agent(session.clone()).await.unwrap();
        manager.remove_session_if_loaded(&session).await.unwrap();
        assert!(!manager.has_session(&session).await);
        manager.remove_session_if_loaded(&session).await.unwrap();
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let temp_dir = TempDir::new().unwrap();
        let manager = Arc::new(create_test_manager(&temp_dir).await);
        let session = String::from("concurrent-test");

        let mut handles = vec![];
        for _ in 0..10 {
            let mgr = Arc::clone(&manager);
            let sess = session.clone();
            handles.push(tokio::spawn(async move {
                mgr.get_or_create_agent(sess).await.unwrap()
            }));
        }

        let agents: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        for agent in &agents[1..] {
            assert!(Arc::ptr_eq(&agents[0], agent));
        }

        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_concurrent_session_creation_race_condition() {
        // Test that concurrent attempts to create the same new session ID
        // result in only one agent being created (tests double-check pattern)
        let temp_dir = TempDir::new().unwrap();
        let manager = Arc::new(create_test_manager(&temp_dir).await);
        let session_id = String::from("race-condition-test");

        // Spawn multiple tasks trying to create the same NEW session simultaneously
        let mut handles = vec![];
        for _ in 0..20 {
            let sess = session_id.clone();
            let mgr_clone = Arc::clone(&manager);
            handles.push(tokio::spawn(async move {
                mgr_clone.get_or_create_agent(sess).await.unwrap()
            }));
        }

        // Collect all agents
        let agents: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        for agent in &agents[1..] {
            assert!(
                Arc::ptr_eq(&agents[0], agent),
                "All concurrent requests should get the same agent"
            );
        }
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_eviction_updates_last_used() {
        // Test that accessing a session updates its last_used timestamp
        // and affects eviction order
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;

        let sessions: Vec<_> = (0..100).map(|i| format!("session-{}", i)).collect();

        for session in &sessions {
            manager.get_or_create_agent(session.clone()).await.unwrap();
            // Small delay to ensure different timestamps
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Access the first session again to update its last_used
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        manager
            .get_or_create_agent(sessions[0].clone())
            .await
            .unwrap();

        // Now create a 101st session - should evict session2 (least recently used)
        let session101 = String::from("session-101");
        manager
            .get_or_create_agent(session101.clone())
            .await
            .unwrap();

        assert!(manager.has_session(&sessions[0]).await);
        assert!(!manager.has_session(&sessions[1]).await);
        assert!(manager.has_session(&session101).await);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_session_error() {
        // Test that removing a nonexistent session returns an error
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        let session = String::from("never-created");

        let result = manager.remove_session(&session).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_remove_session_prunes_creation_lock() {
        // remove_session must drop the per-session creation lock so the
        // HashMap doesn't grow unboundedly.
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        let session = String::from("to-be-removed");

        manager.get_or_create_agent(session.clone()).await.unwrap();
        assert_eq!(manager.creation_locks.lock().await.len(), 1);

        manager.remove_session(&session).await.unwrap();
        assert!(
            manager.creation_locks.lock().await.is_empty(),
            "remove_session must prune the creation lock for the removed session"
        );
    }

    #[tokio::test]
    async fn test_failed_creation_prunes_creation_lock() {
        // Regression test for the Codex review note on PR #9357: when the
        // provider-setup path in `create_agent_locked` returns Err, the
        // outer `get_or_create_agent` must also drop its local Arc clone
        // of the creation lock before pruning.  Otherwise
        // `Arc::strong_count` stays > 1 and the failed session leaks a
        // permanent entry in `creation_locks`.
        use async_trait::async_trait;
        use rmcp::model::Tool;

        use crate::conversation::message::Message;
        use crate::providers::base::{MessageStream, Provider};
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::errors::ProviderError;
        use goose_providers::model::ModelConfig;

        struct FailingProvider;

        #[async_trait]
        impl Provider for FailingProvider {
            fn get_name(&self) -> &str {
                "failing-test-provider"
            }

            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> std::result::Result<MessageStream, ProviderError> {
                Ok(crate::providers::base::stream_from_single_message(
                    Message::assistant().with_text("unused"),
                    ProviderUsage::new("failing-test-provider".into(), Usage::default()),
                ))
            }

            async fn update_mode(
                &self,
                _session_id: &str,
                _mode: GooseMode,
            ) -> std::result::Result<(), ProviderError> {
                Err(ProviderError::ExecutionError(
                    "intentional failure for test".into(),
                ))
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        manager
            .set_default_provider(Arc::new(FailingProvider))
            .await;

        let session_id = String::from("failed-creation-test");
        let result = manager.get_or_create_agent(session_id.clone()).await;

        assert!(
            result.is_err(),
            "expected provider mode-update failure to propagate"
        );
        assert!(
            manager.creation_locks.lock().await.is_empty(),
            "creation_locks must be empty after a failed agent creation"
        );
        assert!(
            !manager.has_session(&session_id).await,
            "failed creation must not insert into the LRU cache"
        );
    }

    #[tokio::test]
    async fn test_failed_provider_restore_is_not_cached_and_can_recover() {
        // Regression test: a session whose record names a provider used to
        // be cached with a provider-less agent when the provider could not
        // be recreated (e.g. expired cloud credentials).  The cache fast
        // path then returned the broken agent forever, so every session
        // load failed with "Provider not set" until the process restarted —
        // even after the credentials were fixed.
        use std::sync::atomic::{AtomicBool, Ordering};

        use async_trait::async_trait;
        use futures::future::BoxFuture;
        use rmcp::model::Tool;

        use crate::conversation::message::Message;
        use crate::providers::base::{
            MessageStream, Provider, ProviderDef, ProviderDescriptor, ProviderMetadata,
        };
        use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
        use goose_providers::model::ModelConfig;

        const PROVIDER_NAME: &str = "restore-failure-test-provider";
        static CREATION_FAILS: AtomicBool = AtomicBool::new(true);

        struct StubProvider;

        #[async_trait]
        impl Provider for StubProvider {
            fn get_name(&self) -> &str {
                PROVIDER_NAME
            }

            async fn stream(
                &self,
                _model_config: &ModelConfig,
                _system: &str,
                _messages: &[Message],
                _tools: &[Tool],
            ) -> std::result::Result<MessageStream, goose_providers::errors::ProviderError>
            {
                Ok(crate::providers::base::stream_from_single_message(
                    Message::assistant().with_text("unused"),
                    ProviderUsage::new(PROVIDER_NAME.into(), Usage::default()),
                ))
            }
        }

        struct RestoreFailureProviderDef;

        impl ProviderDescriptor for RestoreFailureProviderDef {
            fn metadata() -> ProviderMetadata {
                ProviderMetadata::new(
                    PROVIDER_NAME,
                    "Restore Failure Test",
                    "Test-only provider whose creation can be toggled to fail",
                    "test-model",
                    vec!["test-model"],
                    "",
                    vec![],
                )
            }
        }

        impl ProviderDef for RestoreFailureProviderDef {
            type Provider = StubProvider;

            fn from_env(
                _extensions: Vec<crate::config::ExtensionConfig>,
                _tls_config: Option<crate::providers::api_client::TlsConfig>,
            ) -> BoxFuture<'static, anyhow::Result<StubProvider>> {
                Box::pin(async {
                    if CREATION_FAILS.load(Ordering::SeqCst) {
                        Err(anyhow::anyhow!("credentials expired"))
                    } else {
                        Ok(StubProvider)
                    }
                })
            }
        }

        crate::providers::register_provider_for_tests::<RestoreFailureProviderDef>().await;

        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;

        let session = manager
            .session_manager()
            .create_session(
                temp_dir.path().to_path_buf(),
                "restore-failure".into(),
                crate::session::SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();
        manager
            .session_manager()
            .update(&session.id)
            .provider_name(PROVIDER_NAME)
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();

        CREATION_FAILS.store(true, Ordering::SeqCst);
        let result = manager.get_or_create_agent(session.id.clone()).await;
        assert!(
            result.is_err(),
            "failed provider restore must propagate instead of caching a provider-less agent"
        );
        assert!(
            !manager.has_session(&session.id).await,
            "session with failed provider restore must not be cached"
        );

        // Simulate the user fixing the underlying issue (e.g. re-running
        // `aws sso login`): the next attempt must run the restore again and
        // succeed without a process restart.
        CREATION_FAILS.store(false, Ordering::SeqCst);
        let agent = manager
            .get_or_create_agent(session.id.clone())
            .await
            .expect("retry after fixing credentials must succeed");
        assert!(
            agent.provider().await.is_ok(),
            "retry must restore the session provider"
        );
    }

    #[tokio::test]
    async fn test_lru_eviction_prunes_creation_lock() {
        // Sessions can disappear from the LRU cache without going through
        // remove_session.  When that happens the matching creation lock
        // must also be pruned, otherwise long-lived processes that churn
        // through many session IDs would accumulate stale lock entries
        // even though only `max_sessions` agents remain cached.
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let agent_config = AgentConfig::new(
            session_manager,
            PermissionManager::instance(),
            None,
            GooseMode::default(),
            false,
            GoosePlatform::GooseDesktop,
        );
        let manager = AgentManager::new(agent_config, Some(2)).await.unwrap();

        manager.get_or_create_agent("a".into()).await.unwrap();
        manager.get_or_create_agent("b".into()).await.unwrap();
        assert_eq!(manager.creation_locks.lock().await.len(), 2);

        // Inserting a third session evicts the LRU entry ("a").
        manager.get_or_create_agent("c".into()).await.unwrap();

        let locks = manager.creation_locks.lock().await;
        assert_eq!(
            locks.len(),
            2,
            "creation_locks must stay bounded by max_sessions after LRU eviction"
        );
        assert!(
            !locks.contains_key("a"),
            "LRU-evicted session's creation lock should be pruned"
        );
        assert!(locks.contains_key("b"));
        assert!(locks.contains_key("c"));
    }

    #[test_case(GooseMode::Approve ; "approve")]
    #[test_case(GooseMode::Chat ; "chat")]
    #[test_case(GooseMode::SmartApprove ; "smart_approve")]
    #[tokio::test]
    async fn test_agent_inherits_session_mode(mode: GooseMode) {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;

        let session = manager
            .session_manager()
            .create_session(
                temp_dir.path().to_path_buf(),
                "test".into(),
                crate::session::SessionType::User,
                mode,
            )
            .await
            .unwrap();

        let agent = manager.get_or_create_agent(session.id).await.unwrap();
        assert_eq!(agent.goose_mode().await, mode);
    }

    #[tokio::test]
    async fn test_final_output_tool_restored_after_lru_eviction() {
        use crate::agents::final_output_tool::FINAL_OUTPUT_TOOL_NAME;
        use crate::recipe::{Recipe, Response};
        use serde_json::json;

        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
        let agent_config = AgentConfig::new(
            Arc::clone(&session_manager),
            PermissionManager::instance(),
            None,
            GooseMode::default(),
            false,
            GoosePlatform::GooseDesktop,
        );
        let manager = AgentManager::new(agent_config, Some(1)).await.unwrap();

        let session = session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "recipe-session".into(),
                crate::session::SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        let recipe = Recipe {
            version: "1.0.0".into(),
            title: "Test".into(),
            description: "Test recipe".into(),
            response: Some(Response {
                json_schema: Some(json!({
                    "type": "object",
                    "properties": { "result": { "type": "string" } },
                    "required": ["result"]
                })),
            }),
            instructions: None,
            prompt: None,
            extensions: None,
            settings: None,
            activities: None,
            author: None,
            parameters: None,
            sub_recipes: None,
            retry: None,
        };

        session_manager
            .update(&session.id)
            .recipe(Some(recipe))
            .apply()
            .await
            .unwrap();

        // Fill the cache (capacity 1) then evict it
        let agent = manager
            .get_or_create_agent(session.id.clone())
            .await
            .unwrap();
        let tools = agent.list_tools(&session.id, None).await;
        assert!(
            tools
                .iter()
                .any(|t| t.name.as_ref() == FINAL_OUTPUT_TOOL_NAME),
            "final_output_tool must be present on first creation"
        );

        // Evict by adding a second session
        manager
            .get_or_create_agent("evict-trigger".into())
            .await
            .unwrap();
        assert!(
            !manager.has_session(&session.id).await,
            "session should be evicted"
        );

        // Recreate agent via slow path (create_agent_locked)
        let restored_agent = manager
            .get_or_create_agent(session.id.clone())
            .await
            .unwrap();
        let tools = restored_agent.list_tools(&session.id, None).await;
        assert!(
            tools
                .iter()
                .any(|t| t.name.as_ref() == FINAL_OUTPUT_TOOL_NAME),
            "final_output_tool must be restored after LRU eviction"
        );
    }

    #[tokio::test]
    async fn test_session_mode_isolation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = create_test_manager(&temp_dir).await;
        let sm = manager.session_manager();

        let s1 = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "s1".into(),
                crate::session::SessionType::User,
                GooseMode::Approve,
            )
            .await
            .unwrap();
        let s2 = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "s2".into(),
                crate::session::SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();

        let a1 = manager.get_or_create_agent(s1.id).await.unwrap();
        let a2 = manager.get_or_create_agent(s2.id).await.unwrap();

        assert_eq!(a1.goose_mode().await, GooseMode::Approve);
        assert_eq!(a2.goose_mode().await, GooseMode::Auto);
    }
}
