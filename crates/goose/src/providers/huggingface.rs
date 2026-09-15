use super::api_client::{ApiClient, AuthMethod, AuthProvider};
use super::base::{
    ConfigKey, MessageStream, ModelInfo, Provider, ProviderDef, ProviderMetadata,
    DEFAULT_PROVIDER_TIMEOUT_SECS,
};
use super::command_auth::CommandAuthProvider;
use super::huggingface_auth;
use super::openai_compatible::OpenAiCompatibleProvider;
use crate::config::declarative_providers::DeclarativeProviderConfig;
use crate::config::{Config, ConfigError};
use crate::conversation::message::Message;
use crate::session_context::{
    session_id_request_builder, session_id_request_builder_with_header_override,
};
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use rmcp::model::Tool;

pub const HUGGINGFACE_API_HOST: &str = "https://router.huggingface.co/v1";
pub const HUGGINGFACE_DOC_URL: &str = "https://huggingface.co/docs/inference-providers";
pub const HUGGINGFACE_DEFAULT_MODEL: &str = "Qwen/Qwen3-Coder-480B-A35B-Instruct";
pub const HUGGINGFACE_KNOWN_MODELS: &[&str] = &[
    "MiniMaxAI/MiniMax-M2.1",
    "MiniMaxAI/MiniMax-M2.5",
    "MiniMaxAI/MiniMax-M2.7",
    "Qwen/Qwen3-235B-A22B-Thinking",
    "Qwen/Qwen3-Coder-480B-A35B-Instruct",
    "Qwen/Qwen3-Coder-Next",
    "Qwen/Qwen3-Embedding-4B",
    "Qwen/Qwen3-Embedding-8B",
    "Qwen/Qwen3-Next-80B-A3B-Instruct",
    "Qwen/Qwen3-Next-80B-A3B-Thinking",
    "Qwen/Qwen3.5-397B-A17B",
    "XiaomiMiMo/MiMo-V2-Flash",
    "deepseek-ai/DeepSeek-R1",
    "deepseek-ai/DeepSeek-V3.2",
    "deepseek-ai/DeepSeek-V4-Pro",
    "moonshotai/Kimi-K2-Instruct",
    "moonshotai/Kimi-K2-Thinking",
    "moonshotai/Kimi-K2.5",
    "moonshotai/Kimi-K2.6",
    "zai-org/GLM-4.7",
    "zai-org/GLM-4.7-Flash",
    "zai-org/GLM-5",
    "zai-org/GLM-5.1",
];

type QueryParams = Vec<(String, String)>;
type EndpointParts = (String, String, QueryParams);

pub struct HuggingFaceProvider {
    inner: OpenAiCompatibleProvider,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
}

struct HuggingFaceAuthProvider;

#[async_trait::async_trait]
impl AuthProvider for HuggingFaceAuthProvider {
    async fn get_auth_header(&self) -> Result<(String, String)> {
        let token = huggingface_auth::resolve_token_async()
            .await?
            .ok_or_else(missing_token_error)?;
        Ok(("Authorization".to_string(), format!("Bearer {}", token)))
    }
}

impl HuggingFaceProvider {
    pub fn matches_declarative_config(config: &DeclarativeProviderConfig) -> bool {
        config.name == huggingface_auth::HUGGINGFACE_PROVIDER_NAME
            || config.catalog_provider_id.as_deref()
                == Some(huggingface_auth::HUGGINGFACE_PROVIDER_NAME)
    }

    pub fn from_custom_config(
        config: DeclarativeProviderConfig,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> Result<Self> {
        let custom_models = static_models(&config);
        if config.dynamic_models == Some(false) && custom_models.is_none() {
            return Err(anyhow!(
                "Provider '{}' has dynamic_models: false but no static models listed; \
                 at least one entry in `models` is required.",
                config.name
            ));
        }

        config.validate_auth()?;
        let auth_method = match config.auth.as_ref() {
            Some(auth_config) => AuthMethod::Custom(Box::new(CommandAuthProvider::new(
                auth_config,
                "Authorization",
                "Bearer ",
            ))),
            None => custom_auth_method(&config)?,
        };
        let (host, completions_prefix, query_params) =
            openai_compatible_endpoint_parts(&config.base_url, config.base_path.as_deref())?;

        let request_builder = session_id_request_builder_with_header_override(
            config.session_id_header_override.as_deref(),
        )?;
        let timeout_secs = config
            .timeout_seconds
            .unwrap_or(DEFAULT_PROVIDER_TIMEOUT_SECS);
        let mut api_client = ApiClient::with_timeout_and_tls(
            host,
            auth_method,
            std::time::Duration::from_secs(timeout_secs),
            tls_config,
        )?
        .with_request_builder(request_builder)
        .with_query(query_params);

        if let Some(headers) = &config.headers {
            let mut header_map = reqwest::header::HeaderMap::new();
            for (key, value) in headers {
                let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
                let header_value = reqwest::header::HeaderValue::from_str(value)?;
                header_map.insert(header_name, header_value);
            }
            api_client = api_client.with_headers(header_map)?;
        }

        Ok(Self {
            inner: OpenAiCompatibleProvider::new(
                config.name.clone(),
                api_client,
                completions_prefix,
            )
            .with_supports_streaming(config.supports_streaming.unwrap_or(true)),
            custom_models,
            dynamic_models: config.dynamic_models,
        })
    }

    pub async fn cleanup() -> Result<()> {
        huggingface_auth::clear_oauth_token()
    }
}

#[async_trait::async_trait]
impl Provider for HuggingFaceProvider {
    fn get_name(&self) -> &str {
        self.inner.get_name()
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        let configured_limits = self
            .custom_models
            .iter()
            .flatten()
            .filter_map(|model| model.context_limit.map(|limit| (model.name.clone(), limit)));
        goose_providers::context_limit::ContextLimitResolver::new(self.get_name())
            .with_configured_limits(configured_limits)
            .resolve(model, override_limit, || async { Ok(None) })
            .await
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        if let Some(custom_models) = &self.custom_models {
            if self.dynamic_models == Some(false) {
                return Ok(custom_models
                    .iter()
                    .map(|model| model.name.clone())
                    .collect());
            }

            match self.inner.fetch_supported_models().await {
                Ok(models) => return Ok(models),
                Err(e) if e.is_endpoint_not_found() => {
                    tracing::debug!(
                        "Models endpoint not implemented for Hugging Face provider '{}' ({}), using predefined list",
                        self.inner.get_name(),
                        e
                    );
                    return Ok(custom_models
                        .iter()
                        .map(|model| model.name.clone())
                        .collect());
                }
                Err(e) => return Err(e),
            }
        }

        self.inner.fetch_supported_models().await
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        self.inner
            .stream(model_config, system, messages, tools)
            .await
    }
}

impl goose_providers::base::ProviderDescriptor for HuggingFaceProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            huggingface_auth::HUGGINGFACE_PROVIDER_NAME,
            huggingface_auth::HUGGINGFACE_DISPLAY_NAME,
            "Hugging Face Inference Providers via the Hugging Face Router",
            HUGGINGFACE_DEFAULT_MODEL,
            HUGGINGFACE_KNOWN_MODELS.to_vec(),
            HUGGINGFACE_DOC_URL,
            vec![
                ConfigKey::new(
                    huggingface_auth::HUGGINGFACE_TOKEN_SECRET_KEY,
                    true,
                    true,
                    None,
                    true,
                ),
                ConfigKey::new("HF_HOST", false, false, Some(HUGGINGFACE_API_HOST), false),
            ],
        )
        .with_setup(
            crate::providers::catalog::ProviderSetupMetadata::api_key(
                crate::providers::catalog::ProviderSetupGroup::Default,
            )
            .with_docs_url("https://huggingface.co/docs/inference-providers")
            .with_aliases(&["huggingface", "hf"]),
        )
    }
}

impl ProviderDef for HuggingFaceProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(async move {
            let config = Config::global();
            let auth_method =
                refreshable_huggingface_auth_method(huggingface_auth::has_configured_token)?;
            let host: String = config
                .get_param("HF_HOST")
                .unwrap_or_else(|_| HUGGINGFACE_API_HOST.to_string());
            let api_client = ApiClient::new_with_tls(host, auth_method, tls_config)?
                .with_request_builder(session_id_request_builder());

            Ok(Self {
                inner: OpenAiCompatibleProvider::new(
                    huggingface_auth::HUGGINGFACE_PROVIDER_NAME.to_string(),
                    api_client,
                    String::new(),
                ),
                custom_models: None,
                dynamic_models: None,
            })
        })
    }
}

fn missing_token_error() -> anyhow::Error {
    anyhow!(
        "Hugging Face token is not configured. Sign in from Settings > Auth or configure HF_TOKEN."
    )
}

fn configured_api_key(config: &DeclarativeProviderConfig) -> Result<Option<String>> {
    if config.api_key_env.is_empty() {
        return Ok(None);
    }

    match Config::global().get_secret::<String>(&config.api_key_env) {
        Ok(token) => Ok(Some(token)),
        Err(ConfigError::NotFound(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn static_models(config: &DeclarativeProviderConfig) -> Option<Vec<ModelInfo>> {
    (!config.models.is_empty()).then(|| config.models.clone())
}

fn custom_auth_method(config: &DeclarativeProviderConfig) -> Result<AuthMethod> {
    let configured_key = if config.requires_auth {
        configured_api_key(config)?
    } else {
        None
    };
    custom_auth_method_with_provider_token(config.requires_auth, configured_key)
}

fn custom_auth_method_with_provider_token(
    requires_auth: bool,
    provider_token: Option<String>,
) -> Result<AuthMethod> {
    custom_auth_method_from_sources(
        requires_auth,
        provider_token,
        huggingface_auth::has_configured_token,
    )
}

fn custom_auth_method_from_sources(
    requires_auth: bool,
    provider_token: Option<String>,
    has_global_token: impl FnOnce() -> Result<bool>,
) -> Result<AuthMethod> {
    if !requires_auth {
        return Ok(AuthMethod::NoAuth);
    }

    if let Some(token) = provider_token {
        return Ok(AuthMethod::BearerToken(token));
    }

    refreshable_huggingface_auth_method(has_global_token)
}

fn refreshable_huggingface_auth_method(
    has_configured_token: impl FnOnce() -> Result<bool>,
) -> Result<AuthMethod> {
    if !has_configured_token()? {
        return Err(missing_token_error());
    }

    Ok(AuthMethod::Custom(Box::new(HuggingFaceAuthProvider)))
}

fn openai_compatible_endpoint_parts(
    base_url: &str,
    base_path: Option<&str>,
) -> Result<EndpointParts> {
    let url =
        url::Url::parse(base_url).map_err(|e| anyhow!("Invalid base URL '{}': {}", base_url, e))?;
    let mut host = if let Some(port) = url.port() {
        format!(
            "{}://{}:{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            port
        )
    } else {
        format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default())
    };
    let query_params = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();

    if let Some(path) = base_path {
        return Ok((host, completions_prefix(path), query_params));
    }

    let path = url.path().trim_matches('/');
    if path.is_empty() {
        return Ok((host, String::new(), query_params));
    }

    if let Some(parent) = path
        .strip_suffix("/chat/completions")
        .or_else(|| (path == "chat/completions").then_some(""))
    {
        if !parent.is_empty() {
            host.push('/');
            host.push_str(parent);
        }
        return Ok((host, String::new(), query_params));
    }

    host.push('/');
    host.push_str(path);
    Ok((host, String::new(), query_params))
}

fn completions_prefix(path: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return String::new();
    }

    let parent = path
        .strip_suffix("/chat/completions")
        .or_else(|| (path == "chat/completions").then_some(""))
        .unwrap_or(path);

    if parent.is_empty() {
        String::new()
    } else {
        format!("{}/", parent)
    }
}

#[cfg(test)]
mod tests {
    use goose_providers::base::ProviderDescriptor as _;

    use super::*;
    use crate::providers::base::ModelInfo;

    #[test]
    fn metadata_preserves_huggingface_id_and_token_key() {
        let metadata = HuggingFaceProvider::metadata();
        assert_eq!(metadata.name, "huggingface");
        assert_eq!(metadata.display_name, "Hugging Face");
        assert_eq!(metadata.default_model, HUGGINGFACE_DEFAULT_MODEL);
        assert!(metadata
            .config_keys
            .iter()
            .any(|key| key.name == "HF_TOKEN" && key.secret));
    }

    #[test]
    fn declarative_matching_accepts_name_or_catalog_provider_id() {
        let mut config = test_config();
        assert!(!HuggingFaceProvider::matches_declarative_config(&config));

        config.name = "huggingface".to_string();
        assert!(HuggingFaceProvider::matches_declarative_config(&config));

        config.name = "custom_hugging_face".to_string();
        config.catalog_provider_id = Some("huggingface".to_string());
        assert!(HuggingFaceProvider::matches_declarative_config(&config));
    }

    #[test]
    fn endpoint_parts_use_base_url_path_as_api_host() {
        let (host, prefix, query) =
            openai_compatible_endpoint_parts("https://router.huggingface.co/v1?beta=1", None)
                .unwrap();
        assert_eq!(host, "https://router.huggingface.co/v1");
        assert_eq!(prefix, "");
        assert_eq!(query, vec![("beta".to_string(), "1".to_string())]);
    }

    #[test]
    fn endpoint_parts_strip_chat_completions_suffix() {
        let (host, prefix, query) = openai_compatible_endpoint_parts(
            "https://router.huggingface.co/v1/chat/completions",
            None,
        )
        .unwrap();
        assert_eq!(host, "https://router.huggingface.co/v1");
        assert_eq!(prefix, "");
        assert!(query.is_empty());
    }

    #[test]
    fn endpoint_parts_respect_explicit_base_path() {
        let (host, prefix, query) = openai_compatible_endpoint_parts(
            "https://router.huggingface.co",
            Some("v1/chat/completions"),
        )
        .unwrap();
        assert_eq!(host, "https://router.huggingface.co");
        assert_eq!(prefix, "v1/");
        assert!(query.is_empty());
    }

    #[tokio::test]
    async fn custom_provider_returns_static_models_when_dynamic_models_disabled() {
        let mut config = test_config();
        config.requires_auth = false;
        config.dynamic_models = Some(false);
        config.models = vec![
            ModelInfo::new("static-a").with_context_limit(128000),
            ModelInfo::new("static-b").with_context_limit(128000),
        ];

        let provider = HuggingFaceProvider::from_custom_config(config, None).unwrap();

        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["static-a".to_string(), "static-b".to_string()]
        );
        assert_eq!(provider.get_context_limit("static-a", None).await, 128_000);
    }

    #[test]
    fn custom_provider_accepts_command_auth_without_huggingface_token() {
        let mut config = test_config();
        config.api_key_env.clear();
        config.auth = Some(goose_providers::declarative::AuthConfig {
            command: "echo".to_string(),
            args: vec!["token".to_string()],
            refresh_interval: 3600,
            timeout_seconds: None,
            cwd: None,
        });

        HuggingFaceProvider::from_custom_config(config, None).unwrap();
    }

    #[test]
    fn custom_provider_requires_static_models_when_dynamic_models_disabled() {
        let mut config = test_config();
        config.requires_auth = false;
        config.dynamic_models = Some(false);

        let error = match HuggingFaceProvider::from_custom_config(config, None) {
            Ok(_) => panic!("expected dynamic_models: false without static models to fail"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "Provider 'custom_provider' has dynamic_models: false but no static models listed; at least one entry in `models` is required."
        );
    }

    #[test]
    fn custom_auth_method_respects_no_auth_config() {
        let auth_method =
            custom_auth_method_with_provider_token(false, Some("provider-token".to_string()))
                .unwrap();

        assert!(matches!(auth_method, AuthMethod::NoAuth));
    }

    #[test]
    fn custom_auth_method_uses_provider_token_when_auth_is_required() {
        let auth_method =
            custom_auth_method_with_provider_token(true, Some("provider-token".to_string()))
                .unwrap();

        match auth_method {
            AuthMethod::BearerToken(token) => assert_eq!(token, "provider-token"),
            other => panic!("expected bearer token auth, got {other:?}"),
        }
    }

    #[test]
    fn custom_auth_method_uses_refresh_capable_auth_for_global_token() {
        let auth_method = custom_auth_method_from_sources(true, None, || Ok(true)).unwrap();

        assert!(matches!(auth_method, AuthMethod::Custom(_)));
    }

    #[test]
    fn refreshable_huggingface_auth_method_uses_refresh_capable_auth() {
        let auth_method = refreshable_huggingface_auth_method(|| Ok(true)).unwrap();

        assert!(matches!(auth_method, AuthMethod::Custom(_)));
    }

    #[test]
    fn refreshable_huggingface_auth_method_requires_configured_token() {
        let error = refreshable_huggingface_auth_method(|| Ok(false)).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Hugging Face token is not configured. Sign in from Settings > Auth or configure HF_TOKEN."
        );
    }

    #[test]
    fn custom_auth_method_requires_global_token_when_auth_is_required() {
        let error = custom_auth_method_from_sources(true, None, || Ok(false)).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Hugging Face token is not configured. Sign in from Settings > Auth or configure HF_TOKEN."
        );
    }

    fn test_config() -> DeclarativeProviderConfig {
        DeclarativeProviderConfig {
            name: "custom_provider".to_string(),
            engine: crate::config::declarative_providers::ProviderEngine::OpenAI,
            display_name: "Custom Provider".to_string(),
            description: None,
            api_key_env: "CUSTOM_API_KEY".to_string(),
            base_url: HUGGINGFACE_API_HOST.to_string(),
            models: Vec::new(),
            headers: None,
            session_id_header_override: None,
            timeout_seconds: None,
            supports_streaming: Some(true),
            requires_auth: true,
            catalog_provider_id: None,
            base_path: None,
            env_vars: None,
            auth: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            model_doc_link: None,
            setup_steps: vec![],
            toolshim: false,
            preserves_thinking: true,
            emit_clear_thinking: false,
            setup: None,
        }
    }
}
