use anyhow::{bail, Result};
use futures::future::BoxFuture;
use goose_providers::{
    api_client::{ApiClient, AuthMethod, TlsConfig},
    base::{ProviderDescriptor, ProviderMetadata},
    openrouter::OpenRouterProvider,
};
use serde_json::Value;
use std::collections::HashMap;

use crate::{
    config::{Config, ConfigError, ExtensionConfig},
    providers::base::ProviderDef,
};

const OPENROUTER_PARAMETERS_CONFIG_KEY: &str = "OPENROUTER_PARAMETERS";

pub struct OpenRouterProviderDef;

impl ProviderDescriptor for OpenRouterProviderDef {
    fn metadata() -> ProviderMetadata {
        OpenRouterProvider::metadata()
            .with_setup(
                crate::providers::catalog::ProviderSetupMetadata::api_key(
                    crate::providers::catalog::ProviderSetupGroup::Default,
                )
                .with_docs_url("https://openrouter.ai/keys"),
            )
            .with_setup_steps(vec![
                "Go to https://openrouter.ai/settings/keys",
                "Click 'Create' or use an existing API key",
                "Copy the key and paste it above",
            ])
    }
}

impl ProviderDef for OpenRouterProviderDef {
    type Provider = OpenRouterProvider;

    fn from_env(
        _extensions: Vec<ExtensionConfig>,
        tls_config: Option<TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(from_env(tls_config))
    }
}

async fn from_env(tls_config: Option<TlsConfig>) -> Result<OpenRouterProvider> {
    let config = Config::global();
    let api_key: String = config.get_secret("OPENROUTER_API_KEY")?;
    let host: String = config
        .get_param("OPENROUTER_HOST")
        .unwrap_or_else(|_| "https://openrouter.ai".to_string());
    let configured_parameters = configured_openrouter_parameters(config)?;

    let api_client = ApiClient::new_with_tls(host, AuthMethod::BearerToken(api_key), tls_config)?
        .with_request_builder(crate::session_context::session_id_request_builder())
        .with_header("HTTP-Referer", "https://goose-docs.ai")?
        .with_header("X-Title", "goose")?
        .with_header("X-OpenRouter-Categories", "cli-agent,productivity")?;

    Ok(OpenRouterProvider::new(
        api_client,
        configured_parameters,
        Some(Box::new(crate::session_context::current_session_id)),
    ))
}

fn configured_openrouter_parameters(config: &Config) -> Result<Option<HashMap<String, Value>>> {
    match config.get_param::<Value>(OPENROUTER_PARAMETERS_CONFIG_KEY) {
        Ok(raw) => parse_openrouter_parameters(raw).map(Some),
        Err(ConfigError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn parse_openrouter_parameters(raw: Value) -> Result<HashMap<String, Value>> {
    match raw {
        Value::Object(params) => Ok(params.into_iter().collect()),
        Value::String(raw_json) => match serde_json::from_str::<Value>(&raw_json)? {
            Value::Object(params) => Ok(params.into_iter().collect()),
            _ => bail!("{OPENROUTER_PARAMETERS_CONFIG_KEY} must be a JSON object"),
        },
        _ => bail!("{OPENROUTER_PARAMETERS_CONFIG_KEY} must be a JSON object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_includes_parameters_config_key() {
        assert!(OpenRouterProviderDef::metadata()
            .config_keys
            .iter()
            .any(|key| key.name == OPENROUTER_PARAMETERS_CONFIG_KEY));
    }

    #[test]
    fn parses_object_and_json_string_parameters() {
        assert_eq!(
            parse_openrouter_parameters(json!({ "verbosity": "high" })).unwrap()["verbosity"],
            json!("high")
        );
        assert_eq!(
            parse_openrouter_parameters(json!(r#"{"plugins":[{"id":"web"}]}"#)).unwrap()["plugins"],
            json!([{ "id": "web" }])
        );
    }

    #[test]
    fn rejects_non_object_parameters() {
        assert!(parse_openrouter_parameters(json!(r#"["web"]"#))
            .unwrap_err()
            .to_string()
            .contains("OPENROUTER_PARAMETERS must be a JSON object"));
    }
}
