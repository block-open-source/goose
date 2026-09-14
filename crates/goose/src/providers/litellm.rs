use anyhow::Result;
use async_trait::async_trait;
use futures::future::BoxFuture;
use goose_providers::cache_semantics::apply_chat_payload_breakpoints;
use goose_providers::conversation::token_usage::ProviderUsage;
use goose_providers::errors::ProviderError;
use goose_providers::images::ImageFormat;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::api_client::{ApiClient, AuthMethod};
use super::base::{
    ConfigKey, MessageStream, ModelInfo, Provider, ProviderDef, ProviderMetadata,
    DEFAULT_PROVIDER_TIMEOUT_SECS,
};
use super::openai_compatible::{
    handle_response_openai_compat, handle_status, stream_openai_compat,
};
use super::retry::ProviderRetry;
use super::utils::get_model;
use crate::conversation::message::Message;
use goose_providers::model::ModelConfig;
use goose_providers::request_log::{start_log, LoggerHandleExt};
use rmcp::model::Tool;

const LITELLM_PROVIDER_NAME: &str = "litellm";
const LITELLM_DEFAULT_HOST: &str = "http://localhost:4000";
pub const LITELLM_DEFAULT_MODEL: &str = "gpt-4o-mini";
pub const LITELLM_DOC_URL: &str = "https://docs.litellm.ai/docs/";

const MODEL_INFO_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const MODEL_INFO_FAILURE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug)]
enum CachedModelInfo {
    Success(Vec<ModelInfo>),
    Failure(Instant),
}

#[derive(Debug, serde::Serialize)]
pub struct LiteLLMProvider {
    #[serde(skip)]
    api_client: ApiClient,
    base_path: String,
    supports_streaming: bool,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    cached_model_info: tokio::sync::Mutex<Option<CachedModelInfo>>,
}

impl LiteLLMProvider {
    pub async fn from_env(
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> Result<Self> {
        let config = crate::config::Config::global();
        let secrets = config
            .get_secrets("LITELLM_API_KEY", &["LITELLM_CUSTOM_HEADERS"])
            .unwrap_or_default();
        let api_key = secrets.get("LITELLM_API_KEY").cloned().unwrap_or_default();
        let host: String = config
            .get_param("LITELLM_HOST")
            .unwrap_or_else(|_| LITELLM_DEFAULT_HOST.to_string());
        let base_path: String = config
            .get_param("LITELLM_BASE_PATH")
            .unwrap_or_else(|_| "v1/chat/completions".to_string());
        let custom_headers: Option<HashMap<String, String>> = secrets
            .get("LITELLM_CUSTOM_HEADERS")
            .cloned()
            .map(parse_custom_headers);
        let timeout_secs: u64 = config
            .get_param("LITELLM_TIMEOUT")
            .unwrap_or(DEFAULT_PROVIDER_TIMEOUT_SECS);
        let supports_streaming: bool = config
            .get_param("LITELLM_SUPPORTS_STREAMING")
            .unwrap_or(true);

        let auth = if api_key.is_empty() {
            AuthMethod::NoAuth
        } else {
            AuthMethod::BearerToken(api_key)
        };

        let mut api_client = ApiClient::with_timeout_and_tls(
            host,
            auth,
            std::time::Duration::from_secs(timeout_secs),
            tls_config,
        )?
        .with_request_builder(crate::session_context::session_id_request_builder());

        if let Some(headers) = custom_headers {
            let mut header_map = reqwest::header::HeaderMap::new();
            for (key, value) in headers {
                let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
                let header_value = reqwest::header::HeaderValue::from_str(&value)?;
                header_map.insert(header_name, header_value);
            }
            api_client = api_client.with_headers(header_map)?;
        }

        Ok(Self {
            api_client,
            base_path,
            supports_streaming,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info: tokio::sync::Mutex::new(None),
        })
    }

    async fn get_or_fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut cache = self.cached_model_info.lock().await;
        match cache.as_ref() {
            Some(CachedModelInfo::Success(models)) => return Ok(models.clone()),
            Some(CachedModelInfo::Failure(fetched_at))
                if fetched_at.elapsed() < MODEL_INFO_FAILURE_TTL =>
            {
                return Err(ProviderError::RequestFailed(
                    "LiteLLM model metadata is unavailable".to_string(),
                ));
            }
            Some(CachedModelInfo::Failure(_)) | None => {}
        }

        match tokio::time::timeout(MODEL_INFO_DISCOVERY_TIMEOUT, self.fetch_models_from_api()).await
        {
            Ok(Ok(models)) => {
                *cache = Some(CachedModelInfo::Success(models.clone()));
                Ok(models)
            }
            Ok(Err(error)) => {
                *cache = Some(CachedModelInfo::Failure(Instant::now()));
                Err(error)
            }
            Err(_) => {
                *cache = Some(CachedModelInfo::Failure(Instant::now()));
                Err(ProviderError::RequestFailed(
                    "LiteLLM model metadata discovery timed out".to_string(),
                ))
            }
        }
    }

    async fn fetch_models_from_api(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let response = self.api_client.request("model/info").response_get().await?;

        if !response.status().is_success() {
            return Err(ProviderError::RequestFailed(format!(
                "Models endpoint returned status: {}",
                response.status()
            )));
        }

        let response_json: Value = response.json().await.map_err(|e| {
            ProviderError::RequestFailed(format!("Failed to parse models response: {}", e))
        })?;

        let models_data = response_json["data"].as_array().ok_or_else(|| {
            ProviderError::RequestFailed("Missing data field in models response".to_string())
        })?;

        let mut models = Vec::new();
        for model_data in models_data {
            if let Some(model_name) = model_data["model_name"].as_str() {
                if model_name.contains("/*") {
                    continue;
                }

                let model_info = &model_data["model_info"];
                let context_length = model_info["max_input_tokens"]
                    .as_u64()
                    .map(|limit| limit as usize);
                let supports_cache_control = model_info["supports_prompt_caching"].as_bool();

                let mut model_info_obj =
                    ModelInfo::new(model_name).with_optional_context_limit(context_length);
                model_info_obj.supports_cache_control = supports_cache_control;
                models.push(model_info_obj);
            }
        }

        Ok(models)
    }

    async fn post(
        &self,
        model_config: &ModelConfig,
        payload: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        Ok(self
            .api_client
            .request(&self.base_path)
            .model_headers(model_config)?
            .streaming(self.supports_streaming)
            .response_post(payload)
            .await?)
    }

    async fn supports_cache_control(&self, model: &ModelConfig) -> bool {
        if let Ok(models) = self.get_or_fetch_models().await {
            if let Some(model_info) = models.iter().find(|m| m.name == model.model_name) {
                return model_info.supports_cache_control.unwrap_or(false);
            }
        }

        model.model_name.to_lowercase().contains("claude")
    }
}

impl goose_providers::base::ProviderDescriptor for LiteLLMProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            LITELLM_PROVIDER_NAME,
            "LiteLLM",
            "LiteLLM proxy supporting multiple models with automatic prompt caching",
            LITELLM_DEFAULT_MODEL,
            vec![],
            LITELLM_DOC_URL,
            vec![
                ConfigKey::new("LITELLM_API_KEY", true, true, None, true),
                ConfigKey::new(
                    "LITELLM_HOST",
                    true,
                    false,
                    Some(LITELLM_DEFAULT_HOST),
                    true,
                ),
                ConfigKey::new(
                    "LITELLM_BASE_PATH",
                    true,
                    false,
                    Some("v1/chat/completions"),
                    false,
                ),
                ConfigKey::new("LITELLM_CUSTOM_HEADERS", false, true, None, false),
                ConfigKey::new("LITELLM_TIMEOUT", false, false, Some("600"), false),
                ConfigKey::new(
                    "LITELLM_SUPPORTS_STREAMING",
                    false,
                    false,
                    Some("true"),
                    false,
                ),
            ],
        )
        .with_setup(
            crate::providers::catalog::ProviderSetupMetadata::new(
                crate::providers::catalog::ProviderSetupCategory::Model,
                crate::providers::catalog::ProviderSetupMethod::ConfigFields,
                crate::providers::catalog::ProviderSetupGroup::Additional,
            )
            .with_field(
                "LITELLM_HOST",
                "Host URL",
                Some("https://your-proxy.example.com"),
                None,
            )
            .with_field(
                "LITELLM_API_KEY",
                "API Key",
                Some("Paste your API key"),
                None,
            ),
        )
    }
}

impl ProviderDef for LiteLLMProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(Self::from_env(tls_config))
    }
}

#[async_trait]
impl Provider for LiteLLMProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        goose_providers::context_limit::ContextLimitResolver::new(&self.name)
            .resolve(model, override_limit, || async {
                Ok(self
                    .get_or_fetch_models()
                    .await?
                    .iter()
                    .find(|info| info.name == model)
                    .and_then(|info| info.context_limit))
            })
            .await
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let mut payload = goose_providers::formats::openai::create_request(
            model_config,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            self.supports_streaming,
        )?;

        if !model_config.prompt_cache_disabled() && self.supports_cache_control(model_config).await
        {
            apply_chat_payload_breakpoints(&mut payload);
        }

        let mut log = start_log(model_config, &payload)?;

        if self.supports_streaming {
            let response = self
                .with_retry(|| async {
                    let payload_clone = payload.clone();
                    let response = self.post(model_config, &payload_clone).await?;
                    handle_status(response).await
                })
                .await
                .inspect_err(|error| {
                    let _ = log.error(error);
                })?;

            return stream_openai_compat(response, log);
        }

        let response = self
            .with_retry(|| async {
                let payload_clone = payload.clone();
                let response = self.post(model_config, &payload_clone).await?;
                handle_response_openai_compat(response).await
            })
            .await
            .inspect_err(|error| {
                let _ = log.error(error);
            })?;

        let message = goose_providers::formats::openai::response_to_message(&response)?;
        let usage = goose_providers::formats::openai::get_usage(&response);
        let response_model = get_model(&response);
        log.write(&response, Some(&usage))?;
        let provider_usage = ProviderUsage::new(response_model, usage);
        Ok(super::base::stream_from_single_message(
            message,
            provider_usage,
        ))
    }

    fn skip_canonical_filtering(&self) -> bool {
        true
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        let models = self.get_or_fetch_models().await?;
        Ok(models.iter().map(|m| m.name.clone()).collect())
    }
}

fn parse_custom_headers(headers_str: String) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in headers_str.lines() {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use goose_providers::request_log::{install_logger, RequestLogHandle, RequestLogger};
    use serde_json::json;
    use std::sync::{Mutex, Once};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static CAPTURED_REQUEST_LOGS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());
    static INSTALL_REQUEST_LOGGER: Once = Once::new();

    struct CapturingRequestLogger;
    struct CapturingRequestLogHandle(usize);

    impl RequestLogger for CapturingRequestLogger {
        fn start(
            &self,
        ) -> std::result::Result<Box<dyn RequestLogHandle>, Box<dyn std::error::Error + Send + Sync>>
        {
            let mut requests = CAPTURED_REQUEST_LOGS.lock().unwrap();
            requests.push(Vec::new());
            Ok(Box::new(CapturingRequestLogHandle(requests.len() - 1)))
        }
    }

    impl RequestLogHandle for CapturingRequestLogHandle {
        fn write(
            &mut self,
            line: &str,
        ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
            CAPTURED_REQUEST_LOGS.lock().unwrap()[self.0].push(line.to_string());
            Ok(())
        }
    }

    fn install_capturing_request_logger() {
        INSTALL_REQUEST_LOGGER.call_once(|| {
            install_logger(CapturingRequestLogger)
                .expect("test request logger should install once");
        });
    }

    fn captured_request_log(model: &str) -> Vec<Value> {
        CAPTURED_REQUEST_LOGS
            .lock()
            .unwrap()
            .iter()
            .find(|request| {
                request
                    .first()
                    .is_some_and(|line| line.contains(&format!(r#""model_name":"{model}""#)))
            })
            .unwrap_or_else(|| panic!("request log for {model} should exist"))
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn streaming_request_consumes_sse_and_reports_usage() {
        install_capturing_request_logger();
        let server = MockServer::start().await;
        let sse_body = concat!(
            r#"data: {"id":"chatcmpl-litellm","object":"chat.completion.chunk","created":0,"model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"chatcmpl-litellm","object":"chat.completion.chunk","created":0,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            "\n\n",
            r#"data: {"id":"chatcmpl-litellm","object":"chat.completion.chunk","created":0,"model":"test-model","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}"#,
            "\n\n",
            "data: [DONE]",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&server)
            .await;

        let provider = LiteLLMProvider {
            api_client: ApiClient::new_with_tls(server.uri(), AuthMethod::NoAuth, None).unwrap(),
            base_path: "v1/chat/completions".to_string(),
            supports_streaming: true,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info: tokio::sync::Mutex::new(None),
        };
        let model = ModelConfig::new("litellm-streaming-log-test").with_prompt_cache_disabled();

        let mut stream = provider
            .stream(&model, "", &[], &[])
            .await
            .expect("LiteLLM streaming request should consume SSE");
        let mut text = String::new();
        let mut usage = None;
        while let Some(item) = stream.next().await {
            let (message, item_usage) = item.expect("SSE item should parse");
            if let Some(message) = message {
                for content in message.content {
                    text.push_str(&content.to_string());
                }
            }
            if item_usage.is_some() {
                usage = item_usage;
            }
        }

        assert_eq!(text, "hello");
        let usage = usage.expect("SSE usage chunk should be reported");
        assert_eq!(usage.model, "test-model");
        assert_eq!(usage.usage.input_tokens, Some(3));
        assert_eq!(usage.usage.output_tokens, Some(1));
        assert_eq!(usage.usage.total_tokens, Some(4));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({"include_usage": true}));

        let log = captured_request_log("litellm-streaming-log-test");
        assert_eq!(log[0]["input"]["stream"], json!(true));
        assert!(log.iter().skip(1).any(|entry| !entry["data"].is_null()));
        assert!(log
            .iter()
            .skip(1)
            .any(|entry| entry["usage"]["total_tokens"] == json!(4)));
    }

    #[tokio::test]
    async fn non_streaming_request_preserves_json_response_path() {
        install_capturing_request_logger();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-litellm-json",
                "object": "chat.completion",
                "created": 0,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello from JSON"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
            })))
            .mount(&server)
            .await;

        let provider = LiteLLMProvider {
            api_client: ApiClient::new_with_tls(server.uri(), AuthMethod::NoAuth, None).unwrap(),
            base_path: "v1/chat/completions".to_string(),
            supports_streaming: false,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info: tokio::sync::Mutex::new(None),
        };
        let model = ModelConfig::new("litellm-json-log-test").with_prompt_cache_disabled();

        let mut stream = provider
            .stream(&model, "", &[], &[])
            .await
            .expect("LiteLLM JSON response should remain supported");
        let (message, usage) = stream
            .next()
            .await
            .expect("JSON response should yield one item")
            .expect("JSON response item should parse");

        let text = message
            .expect("JSON response should contain a message")
            .content
            .into_iter()
            .map(|content| content.to_string())
            .collect::<String>();
        assert_eq!(text, "hello from JSON");
        let usage = usage.expect("JSON response usage should be reported");
        assert_eq!(usage.model, "test-model");
        assert_eq!(usage.usage.input_tokens, Some(5));
        assert_eq!(usage.usage.output_tokens, Some(3));
        assert_eq!(usage.usage.total_tokens, Some(8));
        assert!(stream.next().await.is_none());

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body.get("stream").is_none());
        assert!(body.get("stream_options").is_none());

        let log = captured_request_log("litellm-json-log-test");
        assert!(log[0]["input"].get("stream").is_none());
        assert!(log.iter().skip(1).any(|entry| !entry["data"].is_null()));
        assert!(log
            .iter()
            .skip(1)
            .any(|entry| entry["usage"]["total_tokens"] == json!(8)));
    }

    #[test]
    fn streaming_config_defaults_to_true() {
        let metadata = <LiteLLMProvider as goose_providers::base::ProviderDescriptor>::metadata();
        let key = metadata
            .config_keys
            .iter()
            .find(|key| key.name == "LITELLM_SUPPORTS_STREAMING")
            .expect("streaming config key should be exposed");

        assert_eq!(key.default.as_deref(), Some("true"));
        assert!(!key.required);
        assert!(!key.secret);
    }

    #[tokio::test]
    async fn context_limit_negative_caches_failed_model_info() {
        let provider = LiteLLMProvider {
            api_client: ApiClient::new_with_tls(
                "http://127.0.0.1:1".to_string(),
                AuthMethod::NoAuth,
                None,
            )
            .unwrap(),
            base_path: "v1/chat/completions".to_string(),
            supports_streaming: true,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info: tokio::sync::Mutex::new(None),
        };

        assert_eq!(
            provider.get_context_limit("unknown-model", None).await,
            goose_providers::model::DEFAULT_CONTEXT_LIMIT
        );
        assert!(matches!(
            provider.cached_model_info.lock().await.as_ref(),
            Some(CachedModelInfo::Failure(_))
        ));
        assert_eq!(
            provider.get_context_limit("unknown-model", None).await,
            goose_providers::model::DEFAULT_CONTEXT_LIMIT
        );
    }

    #[tokio::test]
    async fn expired_failure_allows_model_info_retry() {
        let provider = LiteLLMProvider {
            api_client: ApiClient::new_with_tls(
                "http://127.0.0.1:1".to_string(),
                AuthMethod::NoAuth,
                None,
            )
            .unwrap(),
            base_path: "v1/chat/completions".to_string(),
            supports_streaming: true,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info: tokio::sync::Mutex::new(Some(CachedModelInfo::Failure(
                Instant::now() - MODEL_INFO_FAILURE_TTL,
            ))),
        };

        assert!(provider.get_or_fetch_models().await.is_err());
        assert!(matches!(
            provider.cached_model_info.lock().await.as_ref(),
            Some(CachedModelInfo::Failure(fetched_at))
                if fetched_at.elapsed() < MODEL_INFO_FAILURE_TTL
        ));
    }

    #[tokio::test]
    async fn context_limit_uses_cached_model_info() {
        let cached_model_info =
            tokio::sync::Mutex::new(Some(CachedModelInfo::Success(vec![ModelInfo::new(
                "cached-model",
            )
            .with_context_limit(32_000)])));
        let provider = LiteLLMProvider {
            api_client: ApiClient::new_with_tls(
                "http://127.0.0.1:1".to_string(),
                AuthMethod::NoAuth,
                None,
            )
            .unwrap(),
            base_path: "v1/chat/completions".to_string(),
            supports_streaming: true,
            name: LITELLM_PROVIDER_NAME.to_string(),
            cached_model_info,
        };

        assert_eq!(
            provider.get_context_limit("cached-model", None).await,
            32_000
        );
    }
}
