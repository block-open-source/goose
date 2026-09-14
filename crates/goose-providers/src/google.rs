use crate::api_client::{ApiClient, AuthMethod};
use crate::base::MessageStream;
use crate::conversation::message::Message;
use crate::errors::ProviderError;
use crate::openai_compatible::{handle_status, map_http_error_to_provider_error, sanitize_url};
use crate::retry::ProviderRetry;

use crate::base::{ConfigKey, Provider, ProviderMetadata};
use crate::formats::google::{create_request_with_thinking_budget, response_to_streaming_message};
use crate::model::ModelConfig;
use crate::request_log::{start_log, LoggerHandleExt};
use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures::TryStreamExt;
use rmcp::model::Tool;
use serde_json::Value;
use std::io;
use tokio::pin;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;

pub const GOOGLE_PROVIDER_NAME: &str = "google";
pub const GOOGLE_API_HOST: &str = "https://generativelanguage.googleapis.com";
pub const GOOGLE_DEFAULT_MODEL: &str = "gemini-2.5-pro";
pub const GOOGLE_KNOWN_MODELS: &[&str] = &[
    "gemini-3.6-flash",
    "gemini-3.5-flash",
    "gemini-3.5-flash-lite",
    // Gemini 3 models
    "gemini-3-pro-preview",
    "gemini-3-pro-image-preview",
    // Gemini 2.5 Pro models
    "gemini-2.5-pro",
    "gemini-2.5-pro-preview-tts",
    // Gemini 2.5 Flash models
    "gemini-2.5-flash",
    "gemini-2.5-flash-preview-09-2025",
    "gemini-2.5-flash-image",
    "gemini-2.5-flash-image-preview",
    "gemini-2.5-flash-native-audio-preview-09-2025",
    "gemini-2.5-flash-preview-tts",
    // Gemini 2.5 Flash-Lite models
    "gemini-2.5-flash-lite",
    "gemini-2.5-flash-lite-preview-09-2025",
    // Gemini 2.0 Flash models
    "gemini-2.0-flash",
    "gemini-2.0-flash-001",
    "gemini-2.0-flash-exp",
    "gemini-2.0-flash-preview-image-generation",
    "gemini-2.0-flash-live-001",
    // Gemini 2.0 Flash-Lite models
    "gemini-2.0-flash-lite",
    "gemini-2.0-flash-lite-001",
];

pub const GOOGLE_DOC_URL: &str = "https://ai.google.dev/gemini-api/docs/models";

#[derive(Debug, serde::Serialize)]
pub struct GoogleProvider {
    #[serde(skip)]
    api_client: ApiClient,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    thinking_budget: Option<i32>,
}

impl GoogleProvider {
    pub fn new(
        host: String,
        api_key: String,
        tls_config: Option<crate::api_client::TlsConfig>,
        request_builder: Option<crate::api_client::RequestBuilderDecorator>,
        thinking_budget: Option<i32>,
    ) -> Result<Self> {
        let auth = AuthMethod::ApiKey {
            header_name: "x-goog-api-key".to_string(),
            key: api_key,
        };

        let mut api_client = ApiClient::new_with_tls(host, auth, tls_config)?
            .with_header("Content-Type", "application/json")?;
        if let Some(request_builder) = request_builder {
            api_client = api_client.with_request_builder(request_builder);
        }

        Ok(Self {
            api_client,
            name: GOOGLE_PROVIDER_NAME.to_string(),
            thinking_budget,
        })
    }

    async fn post_stream(
        &self,
        model_config: &ModelConfig,
        payload: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let path = format!(
            "v1beta/models/{}:streamGenerateContent?alt=sse",
            model_config.model_name
        );
        let response = self
            .api_client
            .request(&path)
            .model_headers(model_config)?
            .streaming(true)
            .response_post(payload)
            .await?;
        handle_status(response).await
    }
}

impl crate::base::ProviderDescriptor for GoogleProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            GOOGLE_PROVIDER_NAME,
            "Google Gemini (API Key)",
            "Gemini models from Google AI",
            GOOGLE_DEFAULT_MODEL,
            GOOGLE_KNOWN_MODELS.to_vec(),
            GOOGLE_DOC_URL,
            vec![
                ConfigKey::new("GOOGLE_API_KEY", true, true, None, true),
                ConfigKey::new("GOOGLE_HOST", false, false, Some(GOOGLE_API_HOST), false),
            ],
        )
        .with_setup_steps(vec![
            "Go to https://aistudio.google.com and sign in with your Google account",
            "Click 'Get API key' on the left sidebar",
            "Create a new API key or select an existing one",
            "Copy the key and paste it above",
        ])
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        let response = self
            .api_client
            .request("v1beta/models")
            .response_get()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let url = sanitize_url(response.url().as_str());
            let body = response.text().await.unwrap_or_default();
            let payload = serde_json::from_str::<serde_json::Value>(&body).ok();
            return Err(map_http_error_to_provider_error(status, payload, &url));
        }

        let json: serde_json::Value = response.json().await?;
        let arr = json
            .get("models")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ProviderError::RequestFailed(
                    "Missing 'models' array in Google models response".to_string(),
                )
            })?;
        let mut models: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("name").and_then(|v| v.as_str()))
            .map(|name| name.split('/').next_back().unwrap_or(name).to_string())
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
        let payload = create_request_with_thinking_budget(
            model_config,
            system,
            messages,
            tools,
            self.thinking_budget,
        )?;
        let mut log = start_log(model_config, &payload)?;

        let response = self
            .with_retry(|| async { self.post_stream(model_config, &payload).await })
            .await
            .inspect_err(|e| {
                let _ = log.error(e);
            })?;

        let stream = response.bytes_stream().map_err(io::Error::other);

        Ok(Box::pin(try_stream! {
            let stream_reader = StreamReader::new(stream);
            let framed = FramedRead::new(stream_reader, LinesCodec::new())
                .map_err(anyhow::Error::from);

            let message_stream = response_to_streaming_message(framed);
            pin!(message_stream);
            while let Some(message) = message_stream.next().await {
                let (message, usage) = message.map_err(|e| {
                    e.downcast::<ProviderError>()
                        .unwrap_or_else(ProviderError::stream_decode_error)
                })?;
                if message.is_some() || usage.is_some() {
                    log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
                }
                yield (message, usage);
            }
        }))
    }
}
