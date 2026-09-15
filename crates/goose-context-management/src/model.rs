use std::sync::Arc;

use async_trait::async_trait;
use goose_provider_types::base::Provider;
use goose_provider_types::conversation::message::Message;
use goose_provider_types::conversation::token_usage::ProviderUsage;
use goose_provider_types::errors::ProviderError;
use goose_provider_types::model::ModelConfig;
use rmcp::model::Tool;

/// The single completion call compaction needs. Implementations decide model
/// selection, fallbacks and session plumbing.
#[async_trait]
pub trait CompactionModel: Send + Sync {
    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
    ) -> Result<(Message, ProviderUsage), ProviderError>;

    /// Replays the conversation's own request prefix. The request must stay
    /// cache-compatible with the last routed request (cache on, same thinking
    /// config) so the provider's prompt cache is reused.
    async fn complete_prefix(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<(Message, ProviderUsage), ProviderError>;

    /// Context window the summarization request has to fit. `None` leaves the
    /// request unmeasured, which costs a round trip per overflow.
    async fn context_limit(&self) -> Option<usize> {
        None
    }
}

/// Counts tokens for usage estimation and retained-context reporting.
#[async_trait]
pub trait TokenEstimator: Send + Sync {
    async fn count_chat_tokens(&self, system: &str, messages: &[Message]) -> usize;
    /// Like [`Self::count_chat_tokens`] but including tool schemas; the
    /// default ignores them.
    async fn count_chat_tokens_with_tools(
        &self,
        system: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> usize {
        self.count_chat_tokens(system, messages).await
    }
    async fn count_text_tokens(&self, text: &str) -> usize;
}

pub struct ProviderModel {
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
}

impl ProviderModel {
    pub fn new(provider: Arc<dyn Provider>, model_config: ModelConfig) -> Self {
        Self {
            provider,
            model_config,
        }
    }
}

#[async_trait]
impl CompactionModel for ProviderModel {
    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
    ) -> Result<(Message, ProviderUsage), ProviderError> {
        self.provider
            .complete(&self.model_config, system, messages, &[])
            .await
    }

    async fn complete_prefix(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<(Message, ProviderUsage), ProviderError> {
        self.provider
            .complete(&self.model_config, system, messages, tools)
            .await
    }

    async fn context_limit(&self) -> Option<usize> {
        Some(
            self.provider
                .get_context_limit(
                    &self.model_config.model_name,
                    self.model_config.context_limit,
                )
                .await,
        )
    }
}
