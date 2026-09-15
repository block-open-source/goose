use anyhow::Result;
use futures::future::BoxFuture;

use crate::{
    config::{Config, DeclarativeProviderConfig},
    providers::{
        base::ProviderDef, command_auth::CommandAuthProvider,
        custom_provider_config::ConfigKeyResolver,
    },
    session_context::{
        session_id_request_builder, session_id_request_builder_with_header_override,
    },
};
use goose_providers::{
    anthropic::{self, AnthropicProvider, AnthropicProviderBuilder, ANTHROPIC_API_VERSION},
    api_client::{ApiClient, AuthMethod, TlsConfig},
    base::ProviderDescriptor,
    formats::anthropic::{AnthropicFormatOptions, PrefixMismatchBehavior},
};

pub struct AnthropicProviderDef;

impl ProviderDescriptor for AnthropicProviderDef {
    fn metadata() -> goose_providers::base::ProviderMetadata {
        AnthropicProvider::metadata().with_setup(
            crate::providers::catalog::ProviderSetupMetadata::api_key(
                crate::providers::catalog::ProviderSetupGroup::Default,
            )
            .with_docs_url("https://console.anthropic.com/settings/keys"),
        )
    }
}

impl ProviderDef for AnthropicProviderDef {
    type Provider = AnthropicProvider;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(from_env(tls_config))
    }
}

async fn from_env(
    tls_config: Option<crate::providers::api_client::TlsConfig>,
) -> Result<AnthropicProvider> {
    let config = crate::config::Config::global();
    let api_key: String = config.get_secret("ANTHROPIC_API_KEY")?;
    let host: String = config
        .get_param("ANTHROPIC_HOST")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

    let timeout_secs: u64 = config
        .get_param("ANTHROPIC_TIMEOUT")
        .unwrap_or(crate::providers::base::DEFAULT_PROVIDER_TIMEOUT_SECS);

    let mut format_options = AnthropicFormatOptions::native();
    if let Ok(value) = config.get_param::<String>("ANTHROPIC_PREFIX_MISMATCH_BEHAVIOR") {
        format_options.prefix_mismatch_behavior =
            match value.as_str() {
                "off" => None,
                value => Some(value.parse::<PrefixMismatchBehavior>().map_err(|e| {
                    anyhow::anyhow!("invalid ANTHROPIC_PREFIX_MISMATCH_BEHAVIOR: {e}")
                })?),
            };
    }

    let auth = AuthMethod::ApiKey {
        header_name: "x-api-key".to_string(),
        key: api_key,
    };

    let api_client = ApiClient::with_timeout_and_tls(
        host,
        auth,
        std::time::Duration::from_secs(timeout_secs),
        tls_config,
    )?
    .with_request_builder(session_id_request_builder())
    .with_header("anthropic-version", ANTHROPIC_API_VERSION)?;

    Ok(AnthropicProviderBuilder::new(api_client)
        .format_options(format_options)
        .build())
}

pub fn from_custom_config(
    config: DeclarativeProviderConfig,
    tls_config: Option<TlsConfig>,
) -> Result<AnthropicProvider> {
    let auth_override = config.auth.clone();
    let request_builder = session_id_request_builder_with_header_override(
        config.session_id_header_override.as_deref(),
    )?;
    anthropic::from_declarative_config(config, tls_config, ConfigKeyResolver::new(Config::global()))
        .map(|builder| {
            builder
                .map_api_client(|api_client| {
                    let api_client = api_client.with_request_builder(request_builder);
                    match auth_override {
                        Some(auth_config) => api_client.with_auth(AuthMethod::Custom(Box::new(
                            CommandAuthProvider::new(&auth_config, "x-api-key", ""),
                        ))),
                        None => api_client,
                    }
                })
                .build()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::declarative_providers::{DeclarativeProviderConfig, ProviderEngine};
    use goose_providers::base::{ModelInfo, Provider};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_provider_with_server(
        server_uri: &str,
        custom_models: Option<Vec<String>>,
        dynamic_models: Option<bool>,
    ) -> AnthropicProvider {
        let auth = AuthMethod::ApiKey {
            header_name: "x-api-key".to_string(),
            key: "test-key".to_string(),
        };
        let api_client = ApiClient::new_with_tls(server_uri.to_string(), auth, None)
            .unwrap()
            .with_header("anthropic-version", ANTHROPIC_API_VERSION)
            .unwrap();
        AnthropicProviderBuilder::new(api_client)
            .name("custom_anthropic")
            .custom_models(
                custom_models.map(|models| models.into_iter().map(ModelInfo::new).collect()),
            )
            .dynamic_models(dynamic_models)
            .build()
    }

    fn base_declarative_config(
        models: Vec<ModelInfo>,
        dynamic_models: Option<bool>,
    ) -> DeclarativeProviderConfig {
        DeclarativeProviderConfig {
            name: "custom_anthropic".to_string(),
            engine: ProviderEngine::Anthropic,
            display_name: "Custom Anthropic".to_string(),
            description: None,
            api_key_env: String::new(),
            base_url: "http://localhost:1".to_string(),
            models,
            headers: None,
            session_id_header_override: None,
            timeout_seconds: None,
            supports_streaming: Some(true),
            requires_auth: false,
            catalog_provider_id: None,
            base_path: None,
            env_vars: None,
            auth: None,
            dynamic_models,
            skip_canonical_filtering: false,
            model_doc_link: None,
            setup_steps: vec![],
            toolshim: false,
            preserves_thinking: false,
            emit_clear_thinking: false,
            setup: None,
        }
    }

    #[tokio::test]
    async fn fetch_supported_models_static_only_skips_api() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let provider = make_provider_with_server(
            &server.uri(),
            Some(vec!["m1".to_string(), "m2".to_string()]),
            Some(false),
        );

        let models = provider.fetch_supported_models().await.unwrap();
        assert_eq!(models, vec!["m1".to_string(), "m2".to_string()]);
    }

    #[test]
    fn from_custom_config_rejects_static_only_without_models() {
        let config = base_declarative_config(vec![], Some(false));
        let err = from_custom_config(config, None)
            .err()
            .expect("expected construction error for dynamic_models: false with empty models");
        let msg = err.to_string();
        assert!(
            msg.contains("dynamic_models: false"),
            "error message should mention dynamic_models: false; got: {msg}"
        );
    }

    // Capture the built client's request timeout without exposing a builder
    // getter: map_api_client sees the ApiClient produced by
    // from_declarative_config before it is replaced/decorated.
    fn built_timeout(config: DeclarativeProviderConfig) -> std::time::Duration {
        let captured = std::cell::Cell::new(None);
        anthropic::from_declarative_config(config, None, ConfigKeyResolver::new(Config::global()))
            .expect("provider construction should succeed")
            .map_api_client(|api_client| {
                captured.set(Some(api_client.timeout()));
                api_client
            });
        captured.get().expect("map_api_client should have run")
    }

    #[test]
    fn from_custom_config_honors_explicit_timeout_seconds() {
        let mut config = base_declarative_config(
            vec![ModelInfo::new("m1").with_context_limit(200000)],
            Some(false),
        );
        config.timeout_seconds = Some(120);
        assert_eq!(built_timeout(config), std::time::Duration::from_secs(120));
    }

    #[test]
    fn from_custom_config_defaults_timeout_when_unset() {
        // timeout_seconds: None in base config → 600s default, unchanged
        // behavior for providers that don't set the field.
        let config = base_declarative_config(
            vec![ModelInfo::new("m1").with_context_limit(200000)],
            Some(false),
        );
        assert_eq!(built_timeout(config), std::time::Duration::from_secs(600));
    }
}
