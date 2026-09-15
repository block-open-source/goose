use crate::agents::ExtensionConfig;
use crate::config::Config;
use crate::providers::base::Provider;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default maximum number of turns for task execution
pub const DEFAULT_SUBAGENT_MAX_TURNS: usize = 25;

/// Configuration for task execution with all necessary dependencies
#[derive(Clone)]
pub struct TaskConfig {
    pub provider: Arc<dyn Provider>,
    pub model_config: goose_providers::model::ModelConfig,
    pub parent_session_id: String,
    pub parent_working_dir: PathBuf,
    pub extensions: Vec<ExtensionConfig>,
    pub max_turns: Option<usize>,
}

impl fmt::Debug for TaskConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskConfig")
            .field("provider", &"<dyn Provider>")
            .field("parent_session_id", &self.parent_session_id)
            .field("parent_working_dir", &self.parent_working_dir)
            .field("max_turns", &self.max_turns)
            .field("extensions", &self.extensions)
            .finish()
    }
}

impl TaskConfig {
    pub fn new(
        provider: Arc<dyn Provider>,
        model_config: goose_providers::model::ModelConfig,
        parent_session_id: &str,
        parent_working_dir: &Path,
        extensions: Vec<ExtensionConfig>,
    ) -> Self {
        Self {
            provider,
            model_config: model_config.with_cache_ttl_clamped(),
            parent_session_id: parent_session_id.to_owned(),
            parent_working_dir: parent_working_dir.to_owned(),
            extensions,
            max_turns: Some(
                Config::global()
                    .get_param::<usize>("GOOSE_SUBAGENT_MAX_TURNS")
                    .unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS),
            ),
        }
    }

    pub fn with_max_turns(mut self, max_turns: Option<usize>) -> Self {
        if let Some(turns) = max_turns {
            self.max_turns = Some(turns);
        }
        self
    }
}
