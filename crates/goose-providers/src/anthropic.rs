use crate::api_client::{AuthMethod, TlsConfig};
use crate::base::ProviderDescriptor;
use crate::declarative::{DeclarativeProviderConfig, KeyResolver};
use crate::errors::ProviderError;
use crate::request_log::{start_log, LoggerHandleExt};
use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures::TryStreamExt;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::io;
use tokio::pin;
use tokio_util::io::StreamReader;

use super::api_client::ApiClient;
use super::base::{
    known_models_from_registry, ConfigKey, MessageStream, ModelInfo, Provider, ProviderMetadata,
};
use super::formats::anthropic::{
    block_binding_behavior, create_request_for_model, is_thinking_signature_error,
    response_to_streaming_message, AnthropicFormatOptions, PrefixMismatchBehavior,
    ANTHROPIC_PROVIDER_NAME, INPUT_TRANSFORMATIONS_FIELD, THINKING_BINDING_CONTROLS_BETA,
};
use super::openai_compatible::handle_status;
use super::retry::ProviderRetry;
use crate::conversation::message::Message;
use crate::model::ModelConfig;
use rmcp::model::Tool;

pub const ANTHROPIC_DEFAULT_MODEL: &str = "claude-sonnet-4-5";

const ANTHROPIC_DOC_URL: &str = "https://docs.anthropic.com/en/docs/about-claude/models";
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

// Total-request timeout applied when a declarative provider does not set
// `timeout_seconds`. Matches the OpenAI engine's default (`openai.rs`) and the
// shared `ApiClient` default so behavior is unchanged for providers that leave
// the field unset.
const DEFAULT_ANTHROPIC_TIMEOUT_SECONDS: u64 = 600;

#[derive(serde::Serialize)]
pub struct AnthropicProvider {
    #[serde(skip)]
    api_client: ApiClient,
    supports_streaming: bool,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    #[serde(skip)]
    format_options: AnthropicFormatOptions,
}

/// Builder for [`AnthropicProvider`].
///
/// Exposes every field of the provider so that constructors living outside
/// `anthropic.rs` (e.g. in `anthropic_def.rs`, which lives in the `goose`
/// crate) can assemble a provider without needing direct access to the
/// struct's private fields.
pub struct AnthropicProviderBuilder {
    api_client: ApiClient,
    supports_streaming: bool,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    format_options: AnthropicFormatOptions,
}

impl AnthropicProviderBuilder {
    pub fn new(api_client: ApiClient) -> Self {
        Self {
            api_client,
            supports_streaming: true,
            name: ANTHROPIC_PROVIDER_NAME.to_string(),
            custom_models: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            format_options: AnthropicFormatOptions::default(),
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

    pub fn supports_streaming(mut self, supports_streaming: bool) -> Self {
        self.supports_streaming = supports_streaming;
        self
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

    pub fn format_options(mut self, format_options: AnthropicFormatOptions) -> Self {
        self.format_options = format_options;
        self
    }

    pub fn build(self) -> AnthropicProvider {
        AnthropicProvider {
            api_client: self.api_client,
            supports_streaming: self.supports_streaming,
            name: self.name,
            custom_models: self.custom_models,
            dynamic_models: self.dynamic_models,
            skip_canonical_filtering: self.skip_canonical_filtering,
            format_options: self.format_options,
        }
    }
}

impl AnthropicProvider {
    fn streaming_payload(
        &self,
        model_config: &ModelConfig,
        wire_model: &str,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        format_options: AnthropicFormatOptions,
    ) -> Result<Value, ProviderError> {
        let mut payload = create_request_for_model(
            ANTHROPIC_PROVIDER_NAME,
            model_config,
            wire_model,
            system,
            messages,
            tools,
            format_options,
        )?;
        payload["stream"] = Value::Bool(true);
        Ok(payload)
    }

    async fn post_messages(
        &self,
        model_config: &ModelConfig,
        payload: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let beta_header = beta_header_value(&self.api_client, model_config, payload);
        self.with_retry(|| async {
            let mut request = self
                .api_client
                .request("v1/messages")
                .model_headers(model_config)?;
            if let Some(beta) = &beta_header {
                request = request.header("anthropic-beta", beta)?;
            }
            handle_status(request.streaming(true).response_post(payload).await?).await
        })
        .await
    }

    pub async fn stream_for_model(
        &self,
        model_config: &ModelConfig,
        wire_model: &str,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let payload = self.streaming_payload(
            model_config,
            wire_model,
            system,
            messages,
            tools,
            self.format_options.clone(),
        )?;
        let mut log = start_log(model_config, &payload)?;
        let response = match self.post_messages(model_config, &payload).await {
            Err(ProviderError::RequestFailed(message))
                if is_thinking_signature_error(&message)
                    && !self.format_options.strip_thinking_history
                    && block_binding_behavior(&payload) != Some(PrefixMismatchBehavior::Error) =>
            {
                tracing::warn!(
                    error = %message,
                    "API rejected replayed thinking blocks; retrying once with thinking history stripped. \
                     The rejected blocks stay in the session, so later requests may repeat this retry"
                );
                let _ = log.error(&message);
                let stripped = AnthropicFormatOptions {
                    strip_thinking_history: true,
                    ..self.format_options.clone()
                };
                let payload =
                    self.streaming_payload(model_config, wire_model, system, messages, tools, stripped)?;
                log = start_log(model_config, &payload)?;
                self.post_messages(model_config, &payload).await
            }
            other => other,
        }
        .inspect_err(|e| {
            let _ = log.error(e);
        })?;
        let stream = response.bytes_stream().map_err(io::Error::other);
        Ok(Box::pin(try_stream! {
            let reader = StreamReader::new(stream);
            let framed = tokio_util::codec::FramedRead::new(reader, tokio_util::codec::LinesCodec::new()).map_err(anyhow::Error::from);
            let messages = response_to_streaming_message(framed);
            pin!(messages);
            while let Some(message) = futures::StreamExt::next(&mut messages).await {
                let (message, usage) = message.map_err(ProviderError::from_stream_error)?;
                if let Some(transformations) = usage
                    .as_ref()
                    .and_then(|usage| usage.additional_data.as_ref())
                    .and_then(|data| data.get(INPUT_TRANSFORMATIONS_FIELD))
                {
                    log.write(&json!({ INPUT_TRANSFORMATIONS_FIELD: transformations }), None)?;
                }
                log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
                yield (message, usage);
            }
        }))
    }

    async fn fetch_models_from_api(&self) -> Result<Vec<String>, ProviderError> {
        let response = self.api_client.request("v1/models").response_get().await?;

        if response.status() == StatusCode::NOT_FOUND {
            let body = response.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|p| {
                    p.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "models endpoint not found".to_string());
            return Err(ProviderError::EndpointNotFound(msg));
        }

        let response = handle_status(response).await?;

        let body = response.bytes().await.map_err(|e| {
            ProviderError::NetworkError(format!("Failed to read response body: {}", e))
        })?;
        let json: Value = serde_json::from_slice(&body).map_err(|e| {
            ProviderError::EndpointNotFound(format!("Response body is not valid JSON: {}", e))
        })?;

        if let Some(err_obj) = json.get("error").filter(|error| !error.is_null()) {
            let message = err_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let error_type = err_obj.get("type").and_then(Value::as_str);
            return Err(match error_type {
                Some("authentication_error" | "permission_error") => {
                    ProviderError::Authentication(message)
                }
                Some("rate_limit_error") => ProviderError::RateLimitExceeded {
                    details: message,
                    retry_delay: None,
                },
                Some("billing_error") => ProviderError::CreditsExhausted {
                    details: message,
                    top_up_url: None,
                },
                Some("api_error" | "overloaded_error") => ProviderError::ServerError(message),
                _ => ProviderError::RequestFailed(message),
            });
        }

        let arr = match json.get("data").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => {
                return Err(ProviderError::RequestFailed(
                    "response is not a models payload (missing 'data' array)".into(),
                ));
            }
        };

        let mut models: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        models.sort();
        Ok(models)
    }
}

impl ProviderDescriptor for AnthropicProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::with_models(
            ANTHROPIC_PROVIDER_NAME,
            "Anthropic",
            "Claude and other models from Anthropic",
            ANTHROPIC_DEFAULT_MODEL,
            known_models_from_registry(ANTHROPIC_PROVIDER_NAME),
            ANTHROPIC_DOC_URL,
            vec![
                ConfigKey::new("ANTHROPIC_API_KEY", true, true, None, true),
                ConfigKey::new(
                    "ANTHROPIC_HOST",
                    true,
                    false,
                    Some("https://api.anthropic.com"),
                    false,
                ),
            ],
        )
        .with_setup_steps(vec![
            "Go to https://platform.claude.com/settings/keys",
            "Click 'Create Key'",
            "Copy the key and paste it above",
        ])
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    async fn refresh_credentials(&self) -> Result<(), ProviderError> {
        self.api_client
            .refresh_credentials()
            .await
            .map_err(|error| ProviderError::Authentication(error.to_string()))
    }

    fn skip_canonical_filtering(&self) -> bool {
        self.skip_canonical_filtering
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        let configured_limits = self
            .custom_models
            .iter()
            .flatten()
            .filter_map(|model| model.context_limit.map(|limit| (model.name.clone(), limit)));
        crate::context_limit::ContextLimitResolver::new(&self.name)
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

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        self.stream_for_model(
            model_config,
            &model_config.model_name,
            system,
            messages,
            tools,
        )
        .await
    }
}

fn beta_header_value(
    api_client: &ApiClient,
    model_config: &ModelConfig,
    payload: &Value,
) -> Option<String> {
    let mut features: Vec<String> = model_config
        .request_headers
        .as_ref()
        .and_then(|headers| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("anthropic-beta"))
                .map(|(_, value)| value.as_str())
        })
        .or_else(|| api_client.default_header("anthropic-beta"))
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if block_binding_behavior(payload).is_some()
        && !features.iter().any(|f| f == THINKING_BINDING_CONTROLS_BETA)
    {
        features.push(THINKING_BINDING_CONTROLS_BETA.to_string());
    }
    (!features.is_empty()).then(|| features.join(","))
}

fn format_options_for_provider(
    preserves_thinking: bool,
    emit_clear_thinking: bool,
) -> AnthropicFormatOptions {
    AnthropicFormatOptions {
        preserve_unsigned_thinking: preserves_thinking,
        preserve_thinking_context: preserves_thinking,
        emit_clear_thinking,
        ..Default::default()
    }
}

pub fn from_declarative_config(
    config: DeclarativeProviderConfig,
    tls_config: Option<TlsConfig>,
    key_resolver: impl KeyResolver,
) -> Result<AnthropicProviderBuilder> {
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
        Some(key) if !key.is_empty() => AuthMethod::ApiKey {
            header_name: "x-api-key".to_string(),
            key,
        },
        _ => AuthMethod::NoAuth,
    };

    let format_options =
        format_options_for_provider(config.preserves_thinking, config.emit_clear_thinking);

    let timeout_secs = config
        .timeout_seconds
        .unwrap_or(DEFAULT_ANTHROPIC_TIMEOUT_SECONDS);
    let mut api_client = ApiClient::with_timeout_and_tls(
        config.base_url,
        auth,
        std::time::Duration::from_secs(timeout_secs),
        tls_config,
    )?;

    if let Some(headers) = &config.headers {
        let mut header_map = reqwest::header::HeaderMap::new();
        header_map.insert(
            reqwest::header::HeaderName::from_static("anthropic-version"),
            reqwest::header::HeaderValue::from_static(ANTHROPIC_API_VERSION),
        );
        for (key, value) in headers {
            let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
            let header_value = reqwest::header::HeaderValue::from_str(value)?;
            header_map.insert(header_name, header_value);
        }
        api_client = api_client.with_headers(header_map)?;
    } else {
        api_client = api_client.with_header("anthropic-version", ANTHROPIC_API_VERSION)?;
    }

    let supports_streaming = config.supports_streaming.unwrap_or(true);

    if !supports_streaming {
        return Err(anyhow::anyhow!(
            "Anthropic provider does not support non-streaming mode. All Claude models support streaming. \
            Please remove 'supports_streaming: false' from your provider configuration."
        ));
    }

    Ok(AnthropicProviderBuilder::new(api_client)
        .supports_streaming(supports_streaming)
        .name(config.name.clone())
        .custom_models(custom_models)
        .dynamic_models(config.dynamic_models)
        .skip_canonical_filtering(config.skip_canonical_filtering)
        .format_options(format_options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_client::AuthMethod;
    use crate::conversation::message::MessageContent;
    use serde_json::json;

    struct StubKeyResolver;

    impl crate::declarative::KeyResolver for StubKeyResolver {
        type Error = std::convert::Infallible;

        fn resolve_key(&self, _key: &str) -> Result<String, Self::Error> {
            Ok("test-key".to_string())
        }
    }

    #[test]
    fn zai_provider_config_emits_clear_thinking() {
        let configs = crate::declarative::fixed_provider_configs().unwrap();
        let zai = configs.iter().find(|c| c.name == "zai").cloned().unwrap();
        let builder = from_declarative_config(zai, None, StubKeyResolver).unwrap();

        let mut model = ModelConfig::new("glm-4.7");
        model.max_tokens = Some(64_000);
        let messages = vec![
            Message::assistant().with_content(MessageContent::thinking("internal", "")),
            Message::user().with_text("Continue"),
        ];

        let payload = create_request_for_model(
            "zai",
            &model,
            "glm-4.7",
            "system",
            &messages,
            &[],
            builder.format_options,
        )
        .unwrap();

        assert_eq!(payload["thinking"]["clear_thinking"], false);
    }

    fn make_provider_with_custom_models(
        host: &str,
        custom_models: Vec<String>,
    ) -> AnthropicProvider {
        AnthropicProvider {
            api_client: ApiClient::new_with_tls(host.to_string(), AuthMethod::NoAuth, None)
                .unwrap(),
            supports_streaming: true,
            name: "test-provider".to_string(),
            custom_models: Some(custom_models.into_iter().map(ModelInfo::new).collect()),
            dynamic_models: Some(true),
            skip_canonical_filtering: false,
            format_options: AnthropicFormatOptions::default(),
        }
    }

    #[tokio::test]
    async fn fetch_models_treats_invalid_json_as_endpoint_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html>not a models endpoint</html>"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        let err = provider.fetch_models_from_api().await.unwrap_err();
        assert!(
            err.is_endpoint_not_found(),
            "expected EndpointNotFound, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn fetch_models_treats_missing_data_field_as_request_failed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .expect(1)
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        let err = provider.fetch_models_from_api().await.unwrap_err();
        assert!(
            matches!(err, ProviderError::RequestFailed(_)),
            "expected RequestFailed, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn fetch_supported_models_falls_back_on_invalid_payload() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>error page</html>"))
            .mount(&server)
            .await;

        let predefined = vec![
            "claude-sonnet-4-5".to_string(),
            "claude-haiku-4-5".to_string(),
        ];
        let provider = make_provider_with_custom_models(&server.uri(), predefined.clone());

        let models = provider
            .fetch_supported_models()
            .await
            .expect("should fall back to predefined list on invalid payload");
        assert_eq!(models, predefined);
    }

    #[tokio::test]
    async fn fetch_supported_models_does_not_fall_back_on_missing_data() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        let err = provider.fetch_supported_models().await.unwrap_err();
        assert!(
            matches!(err, ProviderError::RequestFailed(_)),
            "expected RequestFailed to propagate, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn fetch_supported_models_propagates_auth_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "invalid api key"
                }
            })))
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        let err = provider.fetch_supported_models().await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Authentication(_)),
            "expected Authentication error, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn fetch_supported_models_accepts_null_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "model-a"}],
                "error": null
            })))
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["model-a".to_string()]
        );
    }

    #[tokio::test]
    async fn fetch_supported_models_preserves_200_error_type() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "message": "quota exceeded"
                }
            })))
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        assert!(matches!(
            provider.fetch_supported_models().await.unwrap_err(),
            ProviderError::RateLimitExceeded { .. }
        ));
    }

    #[tokio::test]
    async fn fetch_supported_models_propagates_auth_error_from_200_payload() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "invalid api key"
                }
            })))
            .mount(&server)
            .await;

        let provider =
            make_provider_with_custom_models(&server.uri(), vec!["static-model".to_string()]);

        let err = provider.fetch_supported_models().await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Authentication(_)),
            "expected Authentication error, got: {:?}",
            err
        );
    }

    #[test]
    fn beta_header_merges_client_default_with_binding_beta() {
        let client =
            ApiClient::new_with_tls("http://localhost".to_string(), AuthMethod::NoAuth, None)
                .unwrap()
                .with_header("anthropic-beta", "context-1m-2025-08-07")
                .unwrap();
        let payload = json!({
            "thinking": {"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}
        });
        assert_eq!(
            beta_header_value(&client, &ModelConfig::new("claude-opus-5"), &payload).as_deref(),
            Some("context-1m-2025-08-07,thinking-binding-controls-2026-08-01")
        );
    }

    #[test]
    fn metadata_comes_from_the_registry() {
        let metadata = AnthropicProvider::metadata();
        let sonnet = metadata
            .known_models
            .iter()
            .find(|model| model.name == "claude-sonnet-4-5")
            .expect("claude-sonnet-4-5 should come from the catalog");
        assert_eq!(sonnet.context_limit, Some(1_000_000));
        assert!(
            metadata
                .known_models
                .iter()
                .all(|model| !model.name.contains('.')),
            "Anthropic picker ids must be dashed wire names, not dotted catalog names"
        );
    }
}
