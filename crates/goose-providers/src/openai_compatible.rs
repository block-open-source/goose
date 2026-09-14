use crate::conversation::token_usage::{CostSource, ProviderUsage};
use crate::http_status::read_json_response;
use crate::images::ImageFormat;
use anyhow::Error;
use async_stream::try_stream;
use futures::TryStreamExt;
use reqwest::Response;
#[cfg(test)]
use reqwest::StatusCode;
use serde_json::Value;
use tokio::pin;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;

use super::api_client::ApiClient;
use super::base::{stream_from_single_message, MessageStream, Provider};
use super::retry::ProviderRetry;
use crate::conversation::message::Message;
use crate::errors::ProviderError;
use crate::formats::openai::{
    create_request, create_request_for_model_with_options, get_cost, get_usage,
    record_response_metadata, response_to_message, response_to_streaming_message,
    OpenAiFormatOptions,
};
use crate::formats::openai_responses::responses_api_to_streaming_message;
use crate::model::ModelConfig;
use crate::request_log::{start_log, LoggerHandleExt, RequestLogHandle};
use rmcp::model::Tool;

pub struct OpenAiCompatibleProvider {
    name: String,
    /// Client targeted at the base URL (e.g. `https://api.x.ai/v1`)
    api_client: ApiClient,
    /// Path prefix prepended to `chat/completions` (e.g. `"deployments/{name}/"` for Azure).
    completions_prefix: String,
    supports_streaming: bool,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: String, api_client: ApiClient, completions_prefix: String) -> Self {
        Self {
            name,
            api_client,
            completions_prefix,
            supports_streaming: true,
        }
    }

    pub fn with_supports_streaming(mut self, supports_streaming: bool) -> Self {
        self.supports_streaming = supports_streaming;
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn build_request_for_model(
        &self,
        model_config: &ModelConfig,
        wire_model: &str,
        capability_model: &str,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        for_streaming: bool,
    ) -> Result<Value, ProviderError> {
        create_request_for_model_with_options(
            model_config,
            wire_model,
            capability_model,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            for_streaming,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: model_config.supports_vision.unwrap_or_default(),
                ..Default::default()
            },
        )
        .map_err(|e| ProviderError::RequestFailed(format!("Failed to create request: {}", e)))
    }

    pub async fn stream_for_model(
        &self,
        model_config: &ModelConfig,
        wire_model: &str,
        capability_model: &str,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let payload = self.build_request_for_model(
            model_config,
            wire_model,
            capability_model,
            system,
            messages,
            tools,
            self.supports_streaming,
        )?;
        self.stream_payload(model_config, payload).await
    }

    async fn stream_payload(
        &self,
        model_config: &ModelConfig,
        payload: Value,
    ) -> Result<MessageStream, ProviderError> {
        let mut log = start_log(model_config, &payload)?;
        let path = format!("{}chat/completions", self.completions_prefix);
        let response = self
            .with_retry(|| async {
                handle_status(
                    self.api_client
                        .request(&path)
                        .model_headers(model_config)?
                        .streaming(self.supports_streaming)
                        .response_post(&payload)
                        .await?,
                )
                .await
            })
            .await
            .inspect_err(|e| {
                let _ = log.error(e);
            })?;
        if self.supports_streaming {
            stream_openai_compat(response, log)
        } else {
            let json = read_json_response(response).await?;
            let message = response_to_message(&json).map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to parse message: {}", e))
            })?;
            let usage_json = json.get("usage").unwrap_or(&Value::Null);
            let usage_data = get_usage(usage_json);
            let mut usage = ProviderUsage::new(model_config.model_name.clone(), usage_data);
            record_response_metadata(&mut usage, &json);
            if let Some(cost) = get_cost(usage_json) {
                usage = usage.with_cost(cost, CostSource::ProviderReported);
            }
            log.write(
                &serde_json::to_value(&message).unwrap_or_default(),
                Some(&usage.usage),
            )?;
            Ok(stream_from_single_message(message, usage))
        }
    }

    fn build_request(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        for_streaming: bool,
    ) -> Result<Value, ProviderError> {
        create_request(
            model_config,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            for_streaming,
        )
        .map_err(|e| ProviderError::RequestFailed(format!("Failed to create request: {}", e)))
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    async fn refresh_credentials(&self) -> Result<(), ProviderError> {
        self.api_client
            .refresh_credentials()
            .await
            .map_err(|error| ProviderError::Authentication(error.to_string()))
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        let response = self
            .api_client
            .response_get("models")
            .await
            .map_err(|e| ProviderError::RequestFailed(e.to_string()))?;
        let json = handle_response_openai_compat(response).await?;

        if let Some(err_obj) = json.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(ProviderError::Authentication(msg.to_string()));
        }

        let arr = json.get("data").and_then(|v| v.as_array()).ok_or_else(|| {
            ProviderError::RequestFailed("Missing 'data' array in models response".to_string())
        })?;
        let mut models: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        models.sort();
        Ok(models)
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let payload = self.build_request(
            model_config,
            system,
            messages,
            tools,
            self.supports_streaming,
        )?;
        self.stream_payload(model_config, payload).await
    }
}

// Re-exported from the dedicated `http_status` module — these helpers are
// format-agnostic and used across all provider families.
pub use super::http_status::{
    handle_response, handle_status, map_http_error_to_provider_error, sanitize_url,
};

// Legacy alias kept for callers that haven't migrated their import path yet.
pub use super::http_status::handle_response as handle_response_openai_compat;

pub fn stream_openai_compat(
    response: Response,
    mut log: Option<Box<dyn RequestLogHandle>>,
) -> Result<MessageStream, ProviderError> {
    let stream = response.bytes_stream().map_err(std::io::Error::other);

    Ok(Box::pin(try_stream! {
        let stream_reader = StreamReader::new(stream);
        let framed = FramedRead::new(stream_reader, LinesCodec::new())
            .map_err(Error::from);

        let message_stream = response_to_streaming_message(framed);
        pin!(message_stream);
        while let Some(message) = message_stream.next().await {
            let (message, usage) = message.map_err(|e|
                e.downcast::<ProviderError>()
                    .unwrap_or_else(ProviderError::stream_decode_error)
            )?;
            log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
            yield (message, usage);
        }
    }))
}

pub fn stream_responses_compat(
    response: Response,
    mut log: Option<Box<dyn RequestLogHandle>>,
) -> Result<MessageStream, ProviderError> {
    let stream = response.bytes_stream().map_err(std::io::Error::other);

    Ok(Box::pin(try_stream! {
        let stream_reader = StreamReader::new(stream);
        let framed = FramedRead::new(stream_reader, LinesCodec::new())
            .map_err(Error::from);

        let message_stream = responses_api_to_streaming_message(framed);
        pin!(message_stream);
        while let Some(message) = message_stream.next().await {
            let (message, usage) = message.map_err(|e|
                e.downcast::<ProviderError>()
                    .unwrap_or_else(ProviderError::stream_decode_error)
            )?;
            log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
            yield (message, usage);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelConfig;
    use serde_json::json;
    use test_case::test_case;

    #[test_case(
        StatusCode::PAYMENT_REQUIRED,
        Some(json!({"error": {"message": "Insufficient credits to complete this request"}})),
        "CreditsExhausted"
        ; "402 with payload"
    )]
    #[test_case(
        StatusCode::PAYMENT_REQUIRED,
        None,
        "CreditsExhausted"
        ; "402 without payload"
    )]
    #[test_case(
        StatusCode::TOO_MANY_REQUESTS,
        Some(json!({"error": {"message": "Rate limit exceeded"}})),
        "RateLimitExceeded"
        ; "429 rate limit"
    )]
    #[test_case(
        StatusCode::UNAUTHORIZED,
        None,
        "Authentication"
        ; "401 unauthorized"
    )]
    #[test_case(
        StatusCode::BAD_REQUEST,
        Some(json!({"error": {"message": "This request exceeds the maximum context length"}})),
        "ContextLengthExceeded"
        ; "400 context length"
    )]
    #[test_case(
        StatusCode::INTERNAL_SERVER_ERROR,
        None,
        "ServerError"
        ; "500 server error"
    )]
    #[test_case(
        StatusCode::NOT_FOUND,
        None,
        "RequestFailed"
        ; "404 not found"
    )]
    #[test_case(
        StatusCode::NOT_FOUND,
        Some(json!({"error": {"message": "model not available"}})),
        "RequestFailed"
        ; "404 with error payload"
    )]
    fn http_status_maps_to_expected_error(
        status: StatusCode,
        payload: Option<Value>,
        expected_variant: &str,
    ) {
        let err = map_http_error_to_provider_error(status, payload, "http://test/endpoint");
        let actual = err.telemetry_type();
        let expected_telemetry = match expected_variant {
            "CreditsExhausted" => "credits_exhausted",
            "RateLimitExceeded" => "rate_limit",
            "Authentication" => "auth",
            "ContextLengthExceeded" => "context_length",
            "ServerError" => "server",
            "RequestFailed" => "request",
            other => panic!("Unknown variant: {other}"),
        };
        assert_eq!(
            actual, expected_telemetry,
            "Expected {expected_variant}, got error: {err:?}"
        );
    }

    #[test]
    fn build_request_respects_non_streaming_mode() {
        let provider = OpenAiCompatibleProvider::new(
            "test".to_string(),
            ApiClient::new_with_tls(
                "http://localhost".to_string(),
                super::super::api_client::AuthMethod::NoAuth,
                None,
            )
            .unwrap(),
            String::new(),
        )
        .with_supports_streaming(false);

        let model = ModelConfig::new("test-model");
        let payload = provider
            .build_request(&model, "", &[], &[], provider.supports_streaming)
            .unwrap();

        assert_eq!(payload.get("stream"), None);
        assert_eq!(payload.get("stream_options"), None);
    }

    #[tokio::test]
    async fn nonstreaming_completion_accepts_legitimate_response() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": {"role": "assistant", "content": "hello"}
                }]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatibleProvider::new(
            "test".to_string(),
            ApiClient::new_with_tls(server.uri(), crate::api_client::AuthMethod::NoAuth, None)
                .unwrap(),
            String::new(),
        )
        .with_supports_streaming(false);

        let _stream = provider
            .stream(&ModelConfig::new("test-model"), "", &[], &[])
            .await
            .expect("legitimate non-streaming response should be accepted");
    }

    #[tokio::test]
    async fn nonstreaming_completion_rejects_oversized_response_body() {
        use crate::http_status::MAX_PROVIDER_JSON_RESPONSE_BYTES;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "a".repeat(MAX_PROVIDER_JSON_RESPONSE_BYTES + 1)
                    }
                }]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatibleProvider::new(
            "test".to_string(),
            ApiClient::new_with_tls(server.uri(), crate::api_client::AuthMethod::NoAuth, None)
                .unwrap(),
            String::new(),
        )
        .with_supports_streaming(false);

        let err = match provider
            .stream(&ModelConfig::new("test-model"), "", &[], &[])
            .await
        {
            Ok(_) => panic!("oversized response should be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("response body exceeds"),
            "got: {err}"
        );
    }
}
