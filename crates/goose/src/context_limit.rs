use anyhow::Result;
use goose_providers::context_limit::ContextLimitResolver;

use crate::config::Config;
use crate::providers::base::Provider;

pub async fn get_context_limit(provider: &dyn Provider, model: &str) -> Result<usize> {
    let override_limit = Config::global().get_goose_context_limit()?;
    Ok(provider.get_context_limit(model, override_limit).await)
}

pub fn get_local_context_limit(provider_name: &str, model: &str) -> Result<usize> {
    let override_limit = Config::global().get_goose_context_limit()?;
    let mut configured_limits = Vec::new();

    #[cfg(feature = "aws-providers")]
    if provider_name == "aws_bedrock" {
        if let Some(limit) = crate::providers::bedrock::local_context_limit(model) {
            configured_limits.push((model.to_string(), limit));
        }
    }

    #[cfg(feature = "local-inference")]
    if provider_name == "local" {
        if let Some(limit) = crate::providers::local_inference::local_context_limit(model) {
            configured_limits.push((model.to_string(), limit));
        }
    }

    configured_limits.extend(
        crate::config::declarative_providers::load_provider(provider_name)
            .ok()
            .into_iter()
            .flat_map(|loaded| loaded.config.models)
            .filter_map(|model| model.context_limit.map(|limit| (model.name, limit))),
    );

    Ok(ContextLimitResolver::new(provider_name)
        .with_configured_limits(configured_limits)
        .resolve_local(model, override_limit))
}
