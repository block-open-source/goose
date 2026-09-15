use super::api_client::ApiClient;
use super::base::{ConfigKey, MessageStream, ModelInfo, Provider, ProviderMetadata};
use super::openai_compatible::handle_status;
use super::retry::{ProviderRetry, RetryConfig};
use crate::api_client::{AuthMethod, TlsConfig};
use crate::base::ProviderDescriptor;
use crate::conversation::message::Message;
use crate::declarative::{DeclarativeProviderConfig, KeyResolver};
use crate::errors::ProviderError;
use crate::formats::ollama::{create_request, response_to_streaming_message_ollama};
use crate::images::ImageFormat;
use crate::model::ModelConfig;
use crate::request_log::{start_log, LoggerHandleExt, RequestLogHandle};
use anyhow::{Error, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures::TryStreamExt;
use reqwest::{Response, StatusCode};
use rmcp::model::Tool;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::pin;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use url::Url;

pub const OLLAMA_PROVIDER_NAME: &str = "ollama";
pub const OLLAMA_HOST: &str = "localhost";
pub const OLLAMA_TIMEOUT: u64 = 600;
pub const OLLAMA_DEFAULT_PORT: u16 = 11434;
pub const OLLAMA_DEFAULT_MODEL: &str = "qwen3";
pub const OLLAMA_KNOWN_MODELS: &[&str] = &[
    OLLAMA_DEFAULT_MODEL,
    "qwen3-vl",
    "qwen3-coder:30b",
    "qwen3-coder:480b-cloud",
];
pub const OLLAMA_DOC_URL: &str = "https://ollama.com/library";

// Ollama-specific retry config: large models can take 30-120s to load into memory,
// during which Ollama returns 500 errors. Use more retries with gradual backoff
// to wait for the model to become ready.
const OLLAMA_MAX_RETRIES: usize = 10;
const OLLAMA_INITIAL_RETRY_INTERVAL_MS: u64 = 2000;
const OLLAMA_BACKOFF_MULTIPLIER: f64 = 1.5;
const OLLAMA_MAX_RETRY_INTERVAL_MS: u64 = 15_000;

/// Provider settings resolved from `config::Config` at construction time.
///
/// All values that the Ollama provider reads out of the global config are
/// resolved once, up front, and carried here. This keeps the streaming hot
/// path free of config lookups and makes the provider's config dependencies
/// explicit at the construction boundary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OllamaOptions {
    /// Explicit context window override from `GOOSE_INPUT_LIMIT`.
    /// `None` when unset, zero, or invalid; `num_ctx` is then omitted so Ollama
    /// uses its model default.
    pub input_limit: Option<usize>,
    /// Whether to keep `stream_options` in the request (`OLLAMA_STREAM_USAGE`,
    /// default `true`).
    pub stream_usage: bool,
    /// Per-chunk stream timeout in seconds, resolved from
    /// `OLLAMA_STREAM_TIMEOUT` > `GOOSE_STREAM_TIMEOUT` > `OLLAMA_TIMEOUT` >
    /// default (120s).
    pub chunk_timeout_secs: u64,
}

impl Default for OllamaOptions {
    fn default() -> Self {
        Self {
            input_limit: None,
            stream_usage: true,
            chunk_timeout_secs: OLLAMA_DEFAULT_CHUNK_TIMEOUT_SECS,
        }
    }
}

#[derive(serde::Serialize)]
pub struct OllamaProvider {
    #[serde(skip)]
    api_client: ApiClient,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    options: OllamaOptions,
}

pub struct OllamaProviderBuilder {
    api_client: ApiClient,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    options: OllamaOptions,
}

impl OllamaProviderBuilder {
    pub fn new(api_client: ApiClient) -> Self {
        Self {
            api_client,
            name: OLLAMA_PROVIDER_NAME.to_string(),
            custom_models: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            options: OllamaOptions::default(),
        }
    }

    pub fn api_client(mut self, api_client: ApiClient) -> Self {
        self.api_client = api_client;
        self
    }

    pub fn map_api_client(mut self, f: impl FnOnce(ApiClient) -> ApiClient) -> Self {
        self.api_client = f(self.api_client);
        self
    }

    pub fn try_map_api_client(
        mut self,
        f: impl FnOnce(ApiClient) -> Result<ApiClient>,
    ) -> Result<Self> {
        self.api_client = f(self.api_client)?;
        Ok(self)
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn custom_models(mut self, custom_models: Option<Vec<ModelInfo>>) -> Self {
        self.custom_models = custom_models;
        self
    }

    pub fn dynamic_models(mut self, dynamic_models: Option<bool>) -> Self {
        self.dynamic_models = dynamic_models;
        self
    }

    pub fn skip_canonical_filtering(mut self, skip_canonical_filtering: bool) -> Self {
        self.skip_canonical_filtering = skip_canonical_filtering;
        self
    }

    pub fn options(mut self, options: OllamaOptions) -> Self {
        self.options = options;
        self
    }

    pub fn build(self) -> OllamaProvider {
        OllamaProvider {
            api_client: self.api_client,
            name: self.name,
            custom_models: self.custom_models,
            dynamic_models: self.dynamic_models,
            skip_canonical_filtering: self.skip_canonical_filtering,
            options: self.options,
        }
    }
}

impl OllamaProvider {
    pub fn new(
        api_client: ApiClient,
        name: String,
        skip_canonical_filtering: bool,
        options: OllamaOptions,
    ) -> Self {
        OllamaProviderBuilder::new(api_client)
            .name(name)
            .skip_canonical_filtering(skip_canonical_filtering)
            .options(options)
            .build()
    }

    pub fn with_options(mut self, options: OllamaOptions) -> Self {
        self.options = options;
        self
    }

    async fn fetch_models_from_api(&self) -> Result<Vec<String>, ProviderError> {
        fetch_ollama_model_names(&self.api_client)
            .await?
            .ok_or_else(|| ProviderError::RequestFailed("No models array in response".to_string()))
    }
}

pub async fn fetch_ollama_model_names(
    client: &ApiClient,
) -> Result<Option<Vec<String>>, ProviderError> {
    let response = client
        .request("api/tags")
        .response_get()
        .await
        .map_err(|e| ProviderError::RequestFailed(format!("Failed to fetch models: {}", e)))?;

    if response.status() == StatusCode::NOT_FOUND {
        return Err(ProviderError::EndpointNotFound(
            "Ollama models endpoint not found".to_string(),
        ));
    }

    if !response.status().is_success() {
        return Err(ProviderError::RequestFailed(format!(
            "Failed to fetch models: HTTP {}",
            response.status()
        )));
    }

    let json: Value = response
        .json()
        .await
        .map_err(|e| ProviderError::RequestFailed(format!("Failed to parse response: {}", e)))?;

    let Some(models) = json.get("models").and_then(|m| m.as_array()) else {
        return Ok(None);
    };

    let mut names: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();

    names.sort();
    Ok(Some(names))
}

fn resolve_ollama_num_ctx(options: &OllamaOptions) -> Option<usize> {
    options.input_limit
}

fn apply_ollama_options(payload: &mut Value, options: &OllamaOptions, _model_config: &ModelConfig) {
    if let Some(obj) = payload.as_object_mut() {
        // Gate stream_options behind OLLAMA_STREAM_USAGE (default: true).
        // Older Ollama builds that don't support stream_options may stall before
        // emitting any SSE data, blocking until the client timeout (600s).
        // with_line_timeout() only protects after the first line arrives, so
        // users on older builds should set OLLAMA_STREAM_USAGE=false.
        if !options.stream_usage {
            obj.remove("stream_options");
        }

        // Convert max_completion_tokens / max_tokens to Ollama's options.num_predict.
        // Reasoning models emit max_completion_tokens; non-reasoning models emit max_tokens.
        let max_tokens = obj
            .remove("max_completion_tokens")
            .or_else(|| obj.remove("max_tokens"));
        if let Some(max_tokens) = max_tokens {
            let options_value = obj.entry("options").or_insert_with(|| json!({}));
            if let Some(options_obj) = options_value.as_object_mut() {
                options_obj.entry("num_predict").or_insert(max_tokens);
            }
        }

        // Only an explicit GOOSE_INPUT_LIMIT overrides Ollama's num_ctx.
        if let Some(limit) = resolve_ollama_num_ctx(options) {
            let options_value = obj.entry("options").or_insert_with(|| json!({}));
            if let Some(options_obj) = options_value.as_object_mut() {
                options_obj.insert("num_ctx".to_string(), json!(limit));
            }
        }
    }
}

pub fn from_declarative_config(
    config: DeclarativeProviderConfig,
    tls_config: Option<TlsConfig>,
    key_resolver: impl KeyResolver,
) -> Result<OllamaProviderBuilder> {
    let custom_models = if !config.models.is_empty() {
        Some(config.models.clone())
    } else {
        None
    };

    if config.dynamic_models == Some(false) && custom_models.is_none() {
        return Err(anyhow::anyhow!(
            "Provider '{}' has dynamic_models: false but no static models listed; \
             at least one entry in `models` is required.",
            config.name
        ));
    }

    let timeout = Duration::from_secs(config.timeout_seconds.unwrap_or(OLLAMA_TIMEOUT));

    let base_has_scheme =
        config.base_url.starts_with("http://") || config.base_url.starts_with("https://");
    let base = if base_has_scheme {
        config.base_url.clone()
    } else {
        format!("http://{}", config.base_url)
    };

    let mut base_url = Url::parse(&base)
        .map_err(|e| anyhow::anyhow!("Invalid base URL '{}': {}", config.base_url, e))?;

    let is_localhost = matches!(base_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));

    if base_url.port().is_none() && !base_has_scheme && is_localhost {
        base_url
            .set_port(Some(OLLAMA_DEFAULT_PORT))
            .map_err(|_| anyhow::anyhow!("Failed to set default port"))?;
    }

    config.validate_auth()?;

    let api_key = if config.api_key_env.is_empty() {
        None
    } else {
        match key_resolver.resolve_key(config.api_key_env.as_str()) {
            Ok(key) => Some(key),
            Err(err) => {
                if config.requires_auth {
                    anyhow::bail!("missing required key {}: {}", config.api_key_env, err);
                }
                None
            }
        }
    };

    let auth = match api_key {
        Some(key) if !key.is_empty() => AuthMethod::BearerToken(key),
        _ => AuthMethod::NoAuth,
    };

    let mut api_client =
        ApiClient::with_timeout_and_tls(base_url.to_string(), auth, timeout, tls_config)?;

    if let Some(headers) = &config.headers {
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
            let header_value = reqwest::header::HeaderValue::from_str(value)?;
            header_map.insert(header_name, header_value);
        }
        api_client = api_client.with_headers(header_map)?;
    }

    let supports_streaming = config.supports_streaming.unwrap_or(true);

    if !supports_streaming {
        return Err(anyhow::anyhow!(
            "Ollama provider does not support non-streaming mode. All Ollama models support streaming. \
            Please remove 'supports_streaming: false' from your provider configuration."
        ));
    }

    Ok(OllamaProviderBuilder::new(api_client)
        .name(config.name.clone())
        .custom_models(custom_models)
        .dynamic_models(config.dynamic_models)
        .skip_canonical_filtering(config.skip_canonical_filtering))
}

impl ProviderDescriptor for OllamaProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            OLLAMA_PROVIDER_NAME,
            "Ollama",
            "Local open source models",
            OLLAMA_DEFAULT_MODEL,
            OLLAMA_KNOWN_MODELS.to_vec(),
            OLLAMA_DOC_URL,
            vec![
                ConfigKey::new("OLLAMA_HOST", true, false, Some(OLLAMA_HOST), true),
                ConfigKey::new(
                    "OLLAMA_TIMEOUT",
                    false,
                    false,
                    Some(&(OLLAMA_TIMEOUT.to_string())),
                    false,
                ),
            ],
        )
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn skip_canonical_filtering(&self) -> bool {
        self.skip_canonical_filtering
    }

    fn retry_config(&self) -> RetryConfig {
        RetryConfig::new(
            OLLAMA_MAX_RETRIES,
            OLLAMA_INITIAL_RETRY_INTERVAL_MS,
            OLLAMA_BACKOFF_MULTIPLIER,
            OLLAMA_MAX_RETRY_INTERVAL_MS,
        )
        .transient_only()
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let mut payload = create_request(
            model_config,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            true,
        )?;
        apply_ollama_options(&mut payload, &self.options, model_config);
        let mut log = start_log(model_config, &payload)?;

        let response = self
            .with_retry(|| async {
                let resp = self
                    .api_client
                    .request("v1/chat/completions")
                    .model_headers(model_config)?
                    .streaming(true)
                    .response_post(&payload)
                    .await?;
                handle_status(resp).await
            })
            .await
            .inspect_err(|e| {
                let _ = log.error(e);
            })?;
        stream_ollama(response, self.options.chunk_timeout_secs, log)
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        let configured_limits = self
            .custom_models
            .iter()
            .flatten()
            .filter_map(|model| model.context_limit.map(|limit| (model.name.clone(), limit)));
        crate::context_limit::ContextLimitResolver::new(&self.name)
            .with_configured_limits(configured_limits)
            .resolve(model, override_limit, || async {
                Ok(self.options.input_limit)
            })
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

            match self.fetch_models_from_api().await {
                Ok(models) => return Ok(models),
                Err(e) if e.is_endpoint_not_found() => {
                    tracing::debug!(
                        "Models endpoint not implemented for provider '{}' ({}), using predefined list",
                        self.name,
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

        self.fetch_models_from_api().await
    }
}

/// Default per-chunk timeout for Ollama streaming responses (seconds).
/// Configurable via OLLAMA_STREAM_TIMEOUT, GOOSE_STREAM_TIMEOUT, or falls back
/// to OLLAMA_TIMEOUT. Set high to accommodate slower models (CPU inference,
/// large parameter counts, complex reasoning).
pub const OLLAMA_DEFAULT_CHUNK_TIMEOUT_SECS: u64 = 120;

/// Wraps a line stream with a per-item timeout at the raw SSE level.
/// This detects dead connections without false-positive stalls during long
/// tool-call generations where response_to_streaming_message_ollama buffers.
fn with_line_timeout(
    stream: impl futures::Stream<Item = anyhow::Result<String>> + Unpin + Send + 'static,
    timeout_secs: u64,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>> {
    let timeout = Duration::from_secs(timeout_secs);
    Box::pin(try_stream! {
        let mut stream = stream;

        // Allow time-to-first-token to be governed by the request timeout.
        // Only enforce per-chunk timeout after first SSE line arrives.
        match stream.next().await {
            Some(first_item) => yield first_item?,
            None => return,
        }
        loop {
            match tokio::time::timeout(timeout, stream.next()).await {
                Ok(Some(item)) => yield item?,
                Ok(None) => break,
                Err(_) => {
                    Err::<(), anyhow::Error>(anyhow::anyhow!(
                        "Ollama stream stalled: no data received for {}s. \
                         This may indicate the model is overwhelmed by the request payload. \
                         Try a smaller model, reduce the number of tools, or increase the \
                         timeout via OLLAMA_STREAM_TIMEOUT, GOOSE_STREAM_TIMEOUT, or \
                         OLLAMA_TIMEOUT in your config.",
                        timeout_secs
                    ))?;
                }
            }
        }
    })
}

/// Ollama-specific streaming handler with XML tool call fallback.
/// Uses the Ollama format module which buffers text when XML tool calls are detected,
/// preventing duplicate content from being emitted to the UI.
/// Timeout is applied at the raw SSE line level via with_line_timeout so that
/// buffering inside response_to_streaming_message_ollama does not cause false stalls.
fn stream_ollama(
    response: Response,
    chunk_timeout: u64,
    mut log: Option<Box<dyn RequestLogHandle>>,
) -> Result<MessageStream, ProviderError> {
    let stream = response.bytes_stream().map_err(std::io::Error::other);

    Ok(Box::pin(try_stream! {
        let stream_reader = StreamReader::new(stream);
        let framed = FramedRead::new(stream_reader, LinesCodec::new())
            .map_err(Error::from);

        let timed_lines = with_line_timeout(framed, chunk_timeout);
        let message_stream = response_to_streaming_message_ollama(timed_lines);
        pin!(message_stream);

        while let Some(message) = message_stream.next().await {
            let (message, usage) = message.map_err(ProviderError::from_stream_error)?;
            log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
            yield (message, usage);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ModelInfo;

    fn ollama_config(
        dynamic_models: Option<bool>,
        models: Vec<ModelInfo>,
    ) -> DeclarativeProviderConfig {
        ollama_config_with_base_url(dynamic_models, models, "http://localhost:11434")
    }

    fn ollama_config_with_base_url(
        dynamic_models: Option<bool>,
        models: Vec<ModelInfo>,
        base_url: &str,
    ) -> DeclarativeProviderConfig {
        DeclarativeProviderConfig {
            name: "test-ollama".to_string(),
            engine: crate::declarative::ProviderEngine::Ollama,
            display_name: "Test Ollama".to_string(),
            description: None,
            api_key_env: String::new(),
            base_url: base_url.to_string(),
            models,
            headers: None,
            session_id_header_override: None,
            timeout_seconds: None,
            supports_streaming: None,
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
    async fn fetch_supported_models_uses_static_models_when_dynamic_models_false() {
        let provider = from_declarative_config(
            ollama_config(
                Some(false),
                vec![ModelInfo::new("static-model").with_context_limit(4096)],
            ),
            None,
            crate::declarative::EnvKeyResolver,
        )
        .unwrap()
        .build();

        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["static-model".to_string()]
        );
    }

    #[test]
    fn from_custom_config_requires_static_models_when_dynamic_models_false() {
        let err = from_declarative_config(
            ollama_config(Some(false), vec![]),
            None,
            crate::declarative::EnvKeyResolver,
        )
        .err()
        .expect("expected static models validation error");

        assert!(err
            .to_string()
            .contains("dynamic_models: false but no static models listed"));
    }

    #[tokio::test]
    async fn fetch_supported_models_falls_back_to_static_models_on_404() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let provider = from_declarative_config(
            ollama_config_with_base_url(
                None,
                vec![ModelInfo::new("static-model").with_context_limit(4096)],
                &server.uri(),
            ),
            None,
            crate::declarative::EnvKeyResolver,
        )
        .unwrap()
        .build();

        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["static-model".to_string()]
        );
    }

    #[test]
    fn test_apply_ollama_options_uses_input_limit() {
        let options = OllamaOptions {
            input_limit: Some(8192),
            ..Default::default()
        };
        let model_config = ModelConfig::new("qwen3").with_context_limit(Some(16_000));
        let mut payload = json!({});
        apply_ollama_options(&mut payload, &options, &model_config);
        assert_eq!(payload["options"]["num_ctx"], 8192);
    }

    #[test]
    fn test_apply_ollama_options_ignores_context_management_limit() {
        let options = OllamaOptions::default();
        let model_config = ModelConfig::new("qwen3").with_context_limit(Some(12_000));
        let mut payload = json!({});
        apply_ollama_options(&mut payload, &options, &model_config);
        assert!(payload.get("options").is_none());
    }

    #[test]
    fn test_raw_create_request_contains_unsupported_ollama_fields() {
        use crate::formats::ollama::create_request;

        let model_config = ModelConfig::new("llama3.1").with_max_tokens(Some(4096));
        let messages = vec![crate::conversation::message::Message::user().with_text("hi")];

        let payload = create_request(
            &model_config,
            "You are a helpful assistant.",
            &messages,
            &[],
            &ImageFormat::OpenAi,
            true,
        )
        .unwrap();

        assert!(
            payload.get("stream_options").is_some(),
            "create_request should produce stream_options for usage tracking"
        );
        assert!(
            payload.get("max_tokens").is_some(),
            "create_request should produce max_tokens (unsupported by Ollama)"
        );
    }

    #[test]
    fn test_apply_ollama_options_preserves_stream_options_by_default() {
        use crate::formats::ollama::create_request;

        let options = OllamaOptions::default();
        let model_config = ModelConfig::new("llama3.1").with_max_tokens(Some(4096));
        let messages = vec![crate::conversation::message::Message::user().with_text("hi")];

        let mut payload = create_request(
            &model_config,
            "You are a helpful assistant.",
            &messages,
            &[],
            &ImageFormat::OpenAi,
            true,
        )
        .unwrap();

        apply_ollama_options(&mut payload, &options, &model_config);

        assert!(
            payload.get("stream_options").is_some(),
            "stream_options should be preserved by default for usage tracking"
        );
        assert!(
            payload.get("max_tokens").is_none(),
            "max_tokens should be removed for Ollama"
        );
        assert!(
            payload.get("max_completion_tokens").is_none(),
            "max_completion_tokens should be removed for Ollama"
        );
        assert_eq!(
            payload["options"]["num_predict"], 4096,
            "max_tokens should be moved to options.num_predict"
        );
        assert_eq!(payload["stream"], true, "stream field should be preserved");
    }

    #[test]
    fn test_apply_ollama_options_strips_stream_options_when_disabled() {
        use crate::formats::ollama::create_request;

        let options = OllamaOptions {
            input_limit: None,
            stream_usage: false,
            chunk_timeout_secs: 120,
        };
        let model_config = ModelConfig::new("llama3.1").with_max_tokens(Some(4096));
        let messages = vec![crate::conversation::message::Message::user().with_text("hi")];

        let mut payload = create_request(
            &model_config,
            "You are a helpful assistant.",
            &messages,
            &[],
            &ImageFormat::OpenAi,
            true,
        )
        .unwrap();

        apply_ollama_options(&mut payload, &options, &model_config);

        assert!(
            payload.get("stream_options").is_none(),
            "stream_options should be removed when OLLAMA_STREAM_USAGE=false"
        );
    }
}
