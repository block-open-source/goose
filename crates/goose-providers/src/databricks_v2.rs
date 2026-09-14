use crate::formats::anthropic::{AnthropicFormatOptions, ANTHROPIC_PROVIDER_NAME};
use crate::formats::openai::{self, extract_reasoning_effort, is_openai_responses_model};
use crate::http_status::{read_error_body, read_json_response};
use crate::images::ImageFormat;
use anyhow::{anyhow, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures::TryStreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::pin;
use tokio_util::io::StreamReader;

use crate::api_client::{ApiClient, AuthMethod, TlsConfig};
use crate::base::{ConfigKey, MessageStream, Provider, ProviderMetadata};
const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 600;
use crate::conversation::message::Message;
use crate::databricks_auth::{
    DatabricksAuth, DatabricksAuthProvider, DatabricksOauthTokenProvider, DatabricksRefreshHook,
    DatabricksTokenResolver,
};
use crate::errors::ProviderError;
use crate::formats::anthropic;
use crate::formats::openai_responses;
use crate::model::ModelConfig;
use crate::openai_compatible::{handle_status, stream_openai_compat, stream_responses_compat};
use crate::request_log::{start_log, LoggerHandleExt};
use crate::retry::ProviderRetry;
use crate::retry::{
    RetryConfig, DEFAULT_BACKOFF_MULTIPLIER, DEFAULT_INITIAL_RETRY_INTERVAL_MS,
    DEFAULT_MAX_RETRIES, DEFAULT_MAX_RETRY_INTERVAL_MS,
};
use rmcp::model::Tool;

const DATABRICKS_V2_PROVIDER_NAME: &str = "databricks_v2";
const DATABRICKS_V2_DEFAULT_GATEWAY_PATH: &str = "ai-gateway";
const DATABRICKS_V2_ROUTE_SUFFIXES: [&str; 3] = [
    "openai/v1/responses",
    "anthropic/v1/messages",
    "mlflow/v1/chat/completions",
];
const DATABRICKS_V2_LIST_ENDPOINTS_PATH: &str = "api/ai-gateway/v2/endpoints";
const DATABRICKS_V2_LIST_MODEL_SERVICES_PATH: &str = "api/2.1/unity-catalog/model-services";
const DATABRICKS_V2_MODEL_SERVICE_PREFIX: &str = "model-services/";
const DATABRICKS_V2_CATALOG_PAGE_SIZE: usize = 100;
const DATABRICKS_V2_MAX_CATALOG_PAGES: usize = 100;
// Model-services intermittently uses 499 for transient gateway timeouts.
const DATABRICKS_V2_TRANSIENT_GATEWAY_STATUS: u16 = 499;

#[derive(Clone, Copy)]
struct ModelCatalog {
    path: &'static str,
    items_key: &'static str,
    name_prefix: Option<&'static str>,
    view: Option<&'static str>,
    label: &'static str,
}

const DATABRICKS_V2_ENDPOINTS_CATALOG: ModelCatalog = ModelCatalog {
    path: DATABRICKS_V2_LIST_ENDPOINTS_PATH,
    items_key: "endpoints",
    name_prefix: None,
    view: None,
    label: "AI Gateway endpoints",
};

// LIST can include metadata-only services but omits caller-effective grants.
// Inference enforces EXECUTE.
const DATABRICKS_V2_MODEL_SERVICES_CATALOG: ModelCatalog = ModelCatalog {
    path: DATABRICKS_V2_LIST_MODEL_SERVICES_PATH,
    items_key: "model_services",
    name_prefix: Some(DATABRICKS_V2_MODEL_SERVICE_PREFIX),
    view: Some("FULL"),
    label: "model services",
};

pub const DATABRICKS_V2_DEFAULT_MODEL: &str = "databricks-gpt-5-5";
pub const DATABRICKS_V2_KNOWN_MODELS: &[&str] =
    &["databricks-gpt-5-5", "databricks-claude-opus-4-7"];

pub const DATABRICKS_V2_DOC_URL: &str = "https://docs.databricks.com/en/generative-ai/ai-gateway/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabricksV2Route {
    OpenAiResponses,
    AnthropicMessages,
    MlflowChatCompletions,
}

#[derive(Serialize)]
pub struct DatabricksV2Provider {
    #[serde(skip)]
    api_client: ApiClient,
    #[serde(skip)]
    retry_config: RetryConfig,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    token_cache: Arc<Mutex<Option<String>>>,
    #[serde(skip)]
    refresh_hook: Option<DatabricksRefreshHook>,
    #[serde(skip)]
    gateway_path: String,
}

impl DatabricksV2Provider {
    pub async fn cleanup() -> Result<()> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: String,
        auth: DatabricksAuth,
        retry_config: RetryConfig,
        tls_config: Option<TlsConfig>,
        oauth_token_provider: Option<DatabricksOauthTokenProvider>,
        token_resolver: Option<DatabricksTokenResolver>,
        request_builder: Option<crate::api_client::RequestBuilderDecorator>,
        refresh_hook: Option<DatabricksRefreshHook>,
    ) -> Result<Self> {
        let token_cache = Arc::new(Mutex::new(match &auth {
            DatabricksAuth::Token(t) => Some(t.clone()),
            _ => None,
        }));

        let auth_method = AuthMethod::Custom(Box::new(DatabricksAuthProvider {
            auth: auth.clone(),
            token_cache: token_cache.clone(),
            oauth_token_provider,
            token_resolver,
        }));

        let mut api_client = ApiClient::with_timeout_and_tls(
            host,
            auth_method,
            Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS),
            tls_config,
        )?;
        if let Some(request_builder) = request_builder {
            api_client = api_client.with_request_builder(request_builder);
        }

        Ok(Self {
            api_client,
            retry_config,
            name: DATABRICKS_V2_PROVIDER_NAME.to_string(),
            token_cache,
            refresh_hook,
            gateway_path: DATABRICKS_V2_DEFAULT_GATEWAY_PATH.to_string(),
        })
    }

    /// Routes requests through a gateway deployment served under a different
    /// base path. Accepts either the base path (`my-gateway`) or a full route
    /// (`my-gateway/openai/v1/responses`), since callers configure the latter.
    pub fn with_gateway_path(mut self, gateway_path: &str) -> Result<Self> {
        let trimmed = gateway_path.trim().trim_matches('/');
        if trimmed.is_empty() {
            return Err(anyhow!(
                "Databricks gateway path must not be empty; omit it to use the default `{DATABRICKS_V2_DEFAULT_GATEWAY_PATH}`"
            ));
        }
        if trimmed.contains("://") {
            return Err(anyhow!(
                "Databricks gateway path must be a path such as `{DATABRICKS_V2_DEFAULT_GATEWAY_PATH}`, not a URL; configure the workspace URL as the provider host instead"
            ));
        }

        self.gateway_path = DATABRICKS_V2_ROUTE_SUFFIXES
            .iter()
            .find_map(|suffix| trimmed.strip_suffix(suffix))
            .map(|base| base.trim_matches('/'))
            .unwrap_or(trimmed)
            .to_string();

        Ok(self)
    }

    fn route_path(&self, route: DatabricksV2Route) -> String {
        let suffix = match route {
            DatabricksV2Route::OpenAiResponses => "openai/v1/responses",
            DatabricksV2Route::AnthropicMessages => "anthropic/v1/messages",
            DatabricksV2Route::MlflowChatCompletions => "mlflow/v1/chat/completions",
        };
        format!("{}/{suffix}", self.gateway_path)
    }

    pub fn load_retry_config(get_param: impl Fn(&str) -> Option<String>) -> RetryConfig {
        let max_retries = get_param("DATABRICKS_MAX_RETRIES")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_RETRIES);

        let initial_interval_ms = get_param("DATABRICKS_INITIAL_RETRY_INTERVAL_MS")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_INITIAL_RETRY_INTERVAL_MS);

        let backoff_multiplier = get_param("DATABRICKS_BACKOFF_MULTIPLIER")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(DEFAULT_BACKOFF_MULTIPLIER);

        let max_interval_ms = get_param("DATABRICKS_MAX_RETRY_INTERVAL_MS")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_RETRY_INTERVAL_MS);

        RetryConfig::new(
            max_retries,
            initial_interval_ms,
            backoff_multiplier,
            max_interval_ms,
        )
    }

    fn route_for_model(model_name: &str) -> DatabricksV2Route {
        let is_model_service = Self::is_model_service_fqn(model_name);
        let routing_name = if is_model_service {
            model_name.rsplit('.').next().unwrap_or(model_name)
        } else {
            model_name
        };
        let (clean_name, _) = extract_reasoning_effort(routing_name);
        let lower = clean_name.to_lowercase();

        if is_openai_responses_model(&clean_name) || Self::looks_like_gpt5(&lower) {
            DatabricksV2Route::OpenAiResponses
        } else if is_model_service {
            DatabricksV2Route::MlflowChatCompletions
        } else if Self::is_claude_model(&lower) {
            DatabricksV2Route::AnthropicMessages
        } else {
            DatabricksV2Route::MlflowChatCompletions
        }
    }

    fn is_model_service_fqn(model_name: &str) -> bool {
        let Some((catalog, remainder)) = model_name.split_once('.') else {
            return false;
        };
        let Some((schema, service)) = remainder.split_once('.') else {
            return false;
        };
        !catalog.is_empty() && !schema.is_empty() && !service.is_empty()
    }

    fn looks_like_gpt5(model_name: &str) -> bool {
        model_name.contains("gpt-5") || model_name.contains("gpt5")
    }

    fn is_claude_model(model_name: &str) -> bool {
        model_name.contains("claude")
    }

    fn name_looks_chat_capable(name: &str) -> bool {
        if name.to_ascii_lowercase().contains("embedding") {
            return false;
        }
        !name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|segment| {
                segment.eq_ignore_ascii_case("bge") || segment.eq_ignore_ascii_case("gte")
            })
    }

    fn model_service_supports_chat(item: &Value, fallback_name: &str) -> bool {
        let Some(api_types) = item.get("supported_api_types") else {
            // Older workspaces omit capabilities; only the service leaf is safe to inspect.
            return Self::name_looks_chat_capable(fallback_name);
        };
        let Some(api_types) = api_types.as_array() else {
            return false;
        };

        api_types.iter().filter_map(Value::as_str).any(|api_type| {
            api_type.eq_ignore_ascii_case("chat")
                || api_type.eq_ignore_ascii_case("mlflow/v1/chat/completions")
        })
    }

    fn parse_catalog_page(
        json: &Value,
        catalog: &ModelCatalog,
    ) -> Result<(Vec<String>, Option<String>), ProviderError> {
        let items = json
            .get(catalog.items_key)
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ProviderError::RequestFailed(format!(
                    "Unexpected response format from Databricks {} API",
                    catalog.label
                ))
            })?;

        let models: Vec<String> = items
            .iter()
            .filter_map(|item| {
                let name = item.get("name").and_then(Value::as_str)?;
                let (name, is_chat_capable) = match catalog.name_prefix {
                    Some(prefix) => {
                        let name = name.strip_prefix(prefix)?;
                        let leaf = name.rsplit('.').next().unwrap_or(name);
                        (name, Self::model_service_supports_chat(item, leaf))
                    }
                    None => (name, Self::name_looks_chat_capable(name)),
                };
                (!name.is_empty() && is_chat_capable).then(|| name.to_string())
            })
            .collect();

        let next_page_token = json
            .get("next_page_token")
            .and_then(|v| v.as_str())
            .filter(|token| !token.is_empty())
            .map(str::to_string);

        Ok((models, next_page_token))
    }

    async fn stream_openai_responses(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let mut payload =
            openai_responses::create_responses_request(model_config, system, messages, tools)?;
        payload["stream"] = Value::Bool(true);
        let mut log = start_log(model_config, &payload)?;

        let response = self
            .with_retry(|| async {
                let resp = self
                    .api_client
                    .request(&self.route_path(DatabricksV2Route::OpenAiResponses))
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

        stream_responses_compat(response, log)
    }

    async fn stream_mlflow_chat_completions(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let is_model_service = Self::is_model_service_fqn(&model_config.model_name);
        let mut format_config = model_config.clone();
        if is_model_service {
            // Keep UC namespace text out of OpenAI format heuristics.
            format_config.model_name = "model-service".to_string();
        }
        let mut payload = openai::create_request(
            &format_config,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            true,
        )?;
        if is_model_service {
            payload["model"] = Value::String(model_config.model_name.clone());
        }
        if payload.get("max_tokens").is_none() {
            payload["max_tokens"] = Value::from(model_config.max_output_tokens());
        }
        let mut log = start_log(model_config, &payload)?;

        let response = self
            .with_retry(|| async {
                let resp = self
                    .api_client
                    .request(&self.route_path(DatabricksV2Route::MlflowChatCompletions))
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

        stream_openai_compat(response, log)
    }

    async fn stream_anthropic_messages(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let mut payload = anthropic::create_request(
            ANTHROPIC_PROVIDER_NAME,
            model_config,
            system,
            messages,
            tools,
            AnthropicFormatOptions::default(),
        )?;
        payload["stream"] = Value::Bool(true);
        let mut log = start_log(model_config, &payload)?;

        let response = self
            .with_retry(|| async {
                let resp = self
                    .api_client
                    .request(&self.route_path(DatabricksV2Route::AnthropicMessages))
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

        let stream = response.bytes_stream().map_err(io::Error::other);

        Ok(Box::pin(try_stream! {
            let stream_reader = StreamReader::new(stream);
            let framed = tokio_util::codec::FramedRead::new(stream_reader, tokio_util::codec::LinesCodec::new())
                .map_err(anyhow::Error::from);

            let message_stream = anthropic::response_to_streaming_message(framed);
            pin!(message_stream);
            while let Some(message) = futures::StreamExt::next(&mut message_stream).await {
                let (message, usage) = message.map_err(ProviderError::from_stream_error)?;
                log.write(&message, usage.as_ref().map(|f| f.usage).as_ref())?;
                yield (message, usage);
            }
        }))
    }
}

impl crate::base::ProviderDescriptor for DatabricksV2Provider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            DATABRICKS_V2_PROVIDER_NAME,
            "Databricks AI Gateway",
            "Models on Databricks AI Gateway v2",
            DATABRICKS_V2_DEFAULT_MODEL,
            DATABRICKS_V2_KNOWN_MODELS.to_vec(),
            DATABRICKS_V2_DOC_URL,
            vec![
                ConfigKey::new("DATABRICKS_HOST", true, false, None, true),
                ConfigKey::new("DATABRICKS_TOKEN", false, true, None, true),
            ],
        )
    }
}

#[async_trait]
impl Provider for DatabricksV2Provider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn retry_config(&self) -> RetryConfig {
        self.retry_config.clone()
    }

    async fn refresh_credentials(&self) -> Result<(), ProviderError> {
        if let Some(refresh_hook) = &self.refresh_hook {
            refresh_hook();
        }
        *self.token_cache.lock().unwrap() = None;
        Ok(())
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        match Self::route_for_model(&model_config.model_name) {
            DatabricksV2Route::OpenAiResponses => {
                self.stream_openai_responses(model_config, system, messages, tools)
                    .await
            }
            DatabricksV2Route::AnthropicMessages => {
                self.stream_anthropic_messages(model_config, system, messages, tools)
                    .await
            }
            DatabricksV2Route::MlflowChatCompletions => {
                self.stream_mlflow_chat_completions(model_config, system, messages, tools)
                    .await
            }
        }
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        let (endpoint_result, service_result) = tokio::join!(
            self.fetch_model_catalog(&DATABRICKS_V2_ENDPOINTS_CATALOG),
            self.fetch_model_catalog(&DATABRICKS_V2_MODEL_SERVICES_CATALOG),
        );

        let mut names = Vec::new();
        let mut failures = Vec::new();
        let mut any_catalog_succeeded = false;
        for (catalog, result) in [
            (&DATABRICKS_V2_ENDPOINTS_CATALOG, endpoint_result),
            (&DATABRICKS_V2_MODEL_SERVICES_CATALOG, service_result),
        ] {
            match result {
                Ok(models) => {
                    any_catalog_succeeded = true;
                    names.extend(models);
                }
                Err(error) => failures.push((catalog.label, error)),
            }
        }

        if !any_catalog_succeeded {
            let details = failures
                .into_iter()
                .map(|(_, error)| match error {
                    ProviderError::RequestFailed(message) => message,
                    error => error.to_string(),
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProviderError::RequestFailed(details));
        }
        for (label, error) in failures {
            tracing::warn!(catalog = label, %error, "Failed to fetch Databricks model catalog");
        }

        names.sort();
        names.dedup();
        Ok(names)
    }
}

impl DatabricksV2Provider {
    async fn fetch_model_catalog(
        &self,
        catalog: &ModelCatalog,
    ) -> Result<Vec<String>, ProviderError> {
        let ModelCatalog { path, label, .. } = catalog;
        let mut models = Vec::new();
        let mut page_token: Option<String> = None;
        let mut seen_page_tokens = HashSet::new();

        for _ in 0..DATABRICKS_V2_MAX_CATALOG_PAGES {
            let mut path_with_query = format!("{path}?page_size={DATABRICKS_V2_CATALOG_PAGE_SIZE}");
            if let Some(view) = catalog.view {
                path_with_query.push_str(&format!("&view={}", urlencoding::encode(view)));
            }
            if let Some(token) = &page_token {
                path_with_query.push_str(&format!("&page_token={}", urlencoding::encode(token)));
            }

            let json: Value = self
                .with_retry_config(
                    || async {
                        let response = self.api_client.response_get(&path_with_query).await?;
                        if response.status().as_u16() == DATABRICKS_V2_TRANSIENT_GATEWAY_STATUS {
                            let detail = read_error_body(response).await.unwrap_or_default();
                            return Err(ProviderError::ServerError(format!(
                                "Databricks {label} returned {DATABRICKS_V2_TRANSIENT_GATEWAY_STATUS}: {detail}"
                            )));
                        }
                        read_json_response(handle_status(response).await?).await
                    },
                    self.retry_config.clone().transient_only(),
                )
                .await
                .map_err(|error| {
                    ProviderError::RequestFailed(format!(
                        "Failed to fetch Databricks {label}: {error}"
                    ))
                })?;

            let (page_models, next_page_token) = Self::parse_catalog_page(&json, catalog)?;
            models.extend(page_models);

            let Some(next_page_token) = next_page_token else {
                return Ok(models);
            };
            if !seen_page_tokens.insert(next_page_token.clone()) {
                return Err(ProviderError::RequestFailed(format!(
                    "Databricks {label} returned a repeated page token"
                )));
            }
            page_token = Some(next_page_token);
        }

        Err(ProviderError::RequestFailed(format!(
            "Databricks {label} pagination exceeded {DATABRICKS_V2_MAX_CATALOG_PAGES} pages"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_known_model_families() {
        for model in [
            "databricks-gpt-5-5",
            "databricks-gpt5",
            "data_workflow_tools.goose.goose-gpt-6-astra",
        ] {
            assert_eq!(
                DatabricksV2Provider::route_for_model(model),
                DatabricksV2Route::OpenAiResponses,
                "unexpected route for {model}"
            );
        }

        for model in ["databricks-claude-opus-4-7", "databricks-claude-sonnet-4-6"] {
            assert_eq!(
                DatabricksV2Provider::route_for_model(model),
                DatabricksV2Route::AnthropicMessages,
                "unexpected route for {model}"
            );
        }

        assert_eq!(
            DatabricksV2Provider::route_for_model("custom-model"),
            DatabricksV2Route::MlflowChatCompletions
        );
        assert_eq!(
            DatabricksV2Provider::route_for_model("catalog.schema.claude-alias"),
            DatabricksV2Route::MlflowChatCompletions
        );
    }

    #[test]
    fn parses_list_endpoints_response() {
        let json = serde_json::json!({
            "endpoints": [
                {"name": "databricks-claude-opus-4-7"},
                {"name": "databricks-gpt-5-5"},
                {"name": "custom-model"}
            ],
            "next_page_token": "tok"
        });

        let (models, next_page_token) =
            DatabricksV2Provider::parse_catalog_page(&json, &DATABRICKS_V2_ENDPOINTS_CATALOG)
                .unwrap();

        assert_eq!(
            models,
            vec![
                "databricks-claude-opus-4-7".to_string(),
                "databricks-gpt-5-5".to_string(),
                "custom-model".to_string(),
            ]
        );
        assert_eq!(next_page_token.as_deref(), Some("tok"));
    }

    #[test]
    fn errors_when_list_endpoints_response_has_no_endpoints_array() {
        let json = serde_json::json!({"data": []});

        let error =
            DatabricksV2Provider::parse_catalog_page(&json, &DATABRICKS_V2_ENDPOINTS_CATALOG)
                .unwrap_err();

        assert!(matches!(error, ProviderError::RequestFailed(_)));
        assert!(error
            .to_string()
            .contains("Unexpected response format from Databricks AI Gateway endpoints API"));
    }

    #[test]
    fn filters_non_chat_endpoints() {
        let json = serde_json::json!({
            "endpoints": [
                {"name": "databricks-bge-large-en"},
                {"name": "my-gte-small"},
                {"name": "text-embedding-3-large"},
                {"name": "databricks-gpt-5-5"},
                {"name": "custom-model"}
            ]
        });

        let (models, _) =
            DatabricksV2Provider::parse_catalog_page(&json, &DATABRICKS_V2_ENDPOINTS_CATALOG)
                .unwrap();

        assert_eq!(
            models,
            vec!["databricks-gpt-5-5".to_string(), "custom-model".to_string(),]
        );
    }

    #[test]
    fn parses_and_filters_model_services() {
        let json = serde_json::json!({
            "model_services": [
                {
                    "name": "model-services/catalog.schema.vector-search",
                    "supported_api_types": ["mlflow/v1/embeddings"]
                },
                {
                    "name": "model-services/catalog.schema.embedding-assistant",
                    "supported_api_types": ["mlflow/v1/chat/completions"]
                },
                {"name": "model-services/embedding_catalog.gte_schema.chat-model"},
                {"name": "model-services/catalog.schema.bge-embedding"},
                {"name": "model-services/"},
                {"name": "no-prefix"}
            ]
        });

        let (models, _) =
            DatabricksV2Provider::parse_catalog_page(&json, &DATABRICKS_V2_MODEL_SERVICES_CATALOG)
                .unwrap();

        assert_eq!(
            models,
            vec![
                "catalog.schema.embedding-assistant",
                "embedding_catalog.gte_schema.chat-model",
            ]
        );
    }

    #[test]
    fn routes_model_service_fqns_to_mlflow() {
        for model in ["team.claude.kimi-chat", "gpt-5.schema.kimi-chat"] {
            assert_eq!(
                DatabricksV2Provider::route_for_model(model),
                DatabricksV2Route::MlflowChatCompletions,
                "unexpected route for {model}"
            );
        }
    }

    mod gateway_path {
        use super::*;
        use serde_json::json;
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn provider(host: String) -> DatabricksV2Provider {
            DatabricksV2Provider::new(
                host,
                DatabricksAuth::token("test-token".to_string()),
                RetryConfig::new(0, 0, 1.0, 0),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        }

        fn all_route_paths(provider: &DatabricksV2Provider) -> Vec<String> {
            [
                DatabricksV2Route::OpenAiResponses,
                DatabricksV2Route::AnthropicMessages,
                DatabricksV2Route::MlflowChatCompletions,
            ]
            .into_iter()
            .map(|route| provider.route_path(route))
            .collect()
        }

        #[test]
        fn default_matches_the_paths_used_before_the_path_became_configurable() {
            assert_eq!(
                all_route_paths(&provider("https://workspace".to_string())),
                vec![
                    "ai-gateway/openai/v1/responses",
                    "ai-gateway/anthropic/v1/messages",
                    "ai-gateway/mlflow/v1/chat/completions",
                ]
            );
        }

        #[test]
        fn configured_path_is_preserved_for_every_route() {
            let provider = provider("https://workspace".to_string())
                .with_gateway_path("/gateways/team-a/")
                .unwrap();

            assert_eq!(
                all_route_paths(&provider),
                vec![
                    "gateways/team-a/openai/v1/responses",
                    "gateways/team-a/anthropic/v1/messages",
                    "gateways/team-a/mlflow/v1/chat/completions",
                ]
            );
        }

        #[test]
        fn accepts_a_full_route_and_reuses_its_base_for_the_other_routes() {
            let provider = provider("https://workspace".to_string())
                .with_gateway_path("ai-gateway/openai/v1/responses")
                .unwrap();

            assert_eq!(
                all_route_paths(&provider),
                vec![
                    "ai-gateway/openai/v1/responses",
                    "ai-gateway/anthropic/v1/messages",
                    "ai-gateway/mlflow/v1/chat/completions",
                ]
            );
        }

        #[test]
        fn rejects_paths_that_cannot_be_joined_to_a_route() {
            for (input, expected) in [
                ("   ", "must not be empty"),
                ("https://workspace/ai-gateway", "not a URL"),
            ] {
                let err = match provider("https://workspace".to_string()).with_gateway_path(input) {
                    Ok(_) => panic!("{input:?} should be rejected"),
                    Err(err) => err,
                };
                assert!(
                    err.to_string().contains(expected),
                    "error for {input:?} should mention {expected:?}, got: {err}"
                );
            }
        }

        #[tokio::test]
        async fn model_service_gpt_6_uses_responses_route_and_preserves_fqn() {
            let model = "data_workflow_tools.goose.goose-gpt-6-astra";
            let completed = format!(
                r#"data: {{"type":"response.completed","sequence_number":1,"response":{{"id":"resp_1","object":"response","created_at":0,"status":"completed","model":"{model}","output":[],"usage":{{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}}}}"#
            );
            let body = format!("{completed}\n\ndata: [DONE]\n\n");

            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/ai-gateway/openai/v1/responses"))
                .and(body_partial_json(json!({"model": model})))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(body)
                        .append_header("content-type", "text/event-stream"),
                )
                .expect(1)
                .mount(&server)
                .await;

            provider(server.uri())
                .complete(&ModelConfig::new(model), "system", &[], &[])
                .await
                .expect("GPT-6 model service should use the Responses API");
        }

        #[tokio::test]
        async fn responses_api_streams_text_tool_calls_and_usage_over_the_configured_path() {
            let created = r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":0,"status":"in_progress","model":"databricks-gpt-5-5","output":[]}}"#;
            let delta = r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"m1","output_index":0,"content_index":0,"delta":"Hello"}"#;
            let completed = r#"data: {"type":"response.completed","sequence_number":3,"response":{"id":"resp_1","object":"response","created_at":0,"status":"completed","model":"databricks-gpt-5-5","output":[{"type":"function_call","id":"fc_1","call_id":"call_1","name":"shell","arguments":"{\"command\":\"ls\"}"}],"usage":{"input_tokens":11,"output_tokens":3,"total_tokens":14}}}"#;
            let body = format!("{created}\n\n{delta}\n\n{completed}\n\ndata: [DONE]\n\n");

            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/gateways/team-a/openai/v1/responses"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(body)
                        .append_header("content-type", "text/event-stream"),
                )
                .expect(1)
                .mount(&server)
                .await;

            let provider = provider(server.uri())
                .with_gateway_path("gateways/team-a")
                .unwrap();
            let (message, usage) = provider
                .complete(
                    &ModelConfig::new("databricks-gpt-5-5"),
                    "system",
                    &[],
                    &[Tool::new(
                        "shell",
                        "run a shell command",
                        std::sync::Arc::new(
                            json!({"type": "object", "properties": {}})
                                .as_object()
                                .unwrap()
                                .clone(),
                        ),
                    )],
                )
                .await
                .expect("responses stream should decode");

            assert_eq!(message.as_concat_text(), "Hello");
            let tool_request = message
                .content
                .iter()
                .find_map(|content| content.as_tool_request())
                .expect("tool call should decode");
            assert_eq!(tool_request.tool_call.as_ref().unwrap().name, "shell");
            assert_eq!(usage.usage.input_tokens, Some(11));
            assert_eq!(usage.usage.output_tokens, Some(3));
            assert_eq!(usage.usage.total_tokens, Some(14));
        }
    }

    mod fetch_supported_models {
        use super::*;
        use serde_json::json;
        use wiremock::matchers::{method, path, query_param, query_param_is_missing};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn provider(server: &MockServer) -> DatabricksV2Provider {
            provider_with_retry(server, RetryConfig::new(0, 0, 1.0, 0))
        }

        fn provider_with_retry(
            server: &MockServer,
            retry_config: RetryConfig,
        ) -> DatabricksV2Provider {
            DatabricksV2Provider::new(
                server.uri(),
                DatabricksAuth::token("test-token".to_string()),
                retry_config,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        }

        async fn mount_endpoints(server: &MockServer, body: serde_json::Value) {
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_ENDPOINTS_PATH}")))
                .and(query_param("page_size", "100"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(server)
                .await;
        }

        #[tokio::test]
        async fn returns_union_of_both_catalogs_sorted_and_deduplicated() {
            let server = MockServer::start().await;
            mount_endpoints(
                &server,
                json!({"endpoints": [
                    {"name": "databricks-gpt-5-5"},
                    {"name": "catalog.schema.shared-model"}
                ]}),
            )
            .await;
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                .and(query_param("page_size", "100"))
                .and(query_param("view", "FULL"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "model_services": [
                        {"name": "model-services/catalog.schema.shared-model"},
                        {"name": "model-services/data.goose.goose-kimi-k3"}
                    ]
                })))
                .expect(1)
                .mount(&server)
                .await;

            let models = provider(&server).fetch_supported_models().await.unwrap();

            assert_eq!(
                models,
                vec![
                    "catalog.schema.shared-model",
                    "data.goose.goose-kimi-k3",
                    "databricks-gpt-5-5",
                ]
            );
        }

        #[tokio::test]
        async fn paginates_model_services_with_url_encoded_tokens() {
            let server = MockServer::start().await;
            mount_endpoints(&server, json!({"endpoints": [{"name": "endpoint"}]})).await;

            for (page_token, name, next_page_token) in [
                (None, "a.b.c", Some("svc tok%")),
                (Some("svc tok%"), "a.b.d", Some("token-b")),
                (Some("token-b"), "a.b.e", None),
            ] {
                let mut mock = Mock::given(method("GET"))
                    .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                    .and(query_param("page_size", "100"))
                    .and(query_param("view", "FULL"));
                mock = match page_token {
                    Some(page_token) => mock.and(query_param("page_token", page_token)),
                    None => mock.and(query_param_is_missing("page_token")),
                };
                mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "model_services": [{"name": format!("model-services/{name}")}],
                    "next_page_token": next_page_token
                })))
                .expect(1)
                .mount(&server)
                .await;
            }

            let models = provider(&server).fetch_supported_models().await.unwrap();

            assert_eq!(models, vec!["a.b.c", "a.b.d", "a.b.e", "endpoint"]);
        }

        #[tokio::test]
        async fn rejects_model_service_page_token_cycles() {
            let server = MockServer::start().await;
            for page_token in [None, Some("token-a")] {
                let mut mock = Mock::given(method("GET"))
                    .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")));
                mock = match page_token {
                    Some(page_token) => mock.and(query_param("page_token", page_token)),
                    None => mock.and(query_param_is_missing("page_token")),
                };
                mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "model_services": [],
                    "next_page_token": "token-a"
                })))
                .expect(1)
                .mount(&server)
                .await;
            }

            let error = provider(&server)
                .fetch_model_catalog(&DATABRICKS_V2_MODEL_SERVICES_CATALOG)
                .await
                .unwrap_err();

            assert!(error.to_string().contains("repeated page token"));
        }

        #[tokio::test]
        async fn does_not_retry_permanent_catalog_failures() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                    "message": "not available"
                })))
                .expect(1)
                .mount(&server)
                .await;

            let error = provider_with_retry(&server, RetryConfig::new(3, 0, 1.0, 0))
                .fetch_model_catalog(&DATABRICKS_V2_MODEL_SERVICES_CATALOG)
                .await
                .unwrap_err();

            assert!(error.to_string().contains("not available"));
        }

        #[tokio::test]
        async fn retries_transient_catalog_failures() {
            for status in [500, DATABRICKS_V2_TRANSIENT_GATEWAY_STATUS] {
                let server = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                    .respond_with(ResponseTemplate::new(status))
                    .up_to_n_times(1)
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "model_services": []
                    })))
                    .expect(1)
                    .mount(&server)
                    .await;

                let models = provider_with_retry(&server, RetryConfig::new(1, 0, 1.0, 0))
                    .fetch_model_catalog(&DATABRICKS_V2_MODEL_SERVICES_CATALOG)
                    .await
                    .unwrap();

                assert!(models.is_empty());
            }
        }

        #[tokio::test]
        async fn returns_endpoints_when_services_fail() {
            let server = MockServer::start().await;
            mount_endpoints(&server, json!({"endpoints": [{"name": "only-endpoint"}]})).await;
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "boom"})))
                .mount(&server)
                .await;

            let models = provider(&server).fetch_supported_models().await.unwrap();

            assert_eq!(models, vec!["only-endpoint"]);
        }

        #[tokio::test]
        async fn errors_when_both_catalogs_fail() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_ENDPOINTS_PATH}")))
                .respond_with(
                    ResponseTemplate::new(500).set_body_json(json!({"message": "endpoints down"})),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/{DATABRICKS_V2_LIST_MODEL_SERVICES_PATH}")))
                .respond_with(
                    ResponseTemplate::new(500).set_body_json(json!({"message": "services down"})),
                )
                .mount(&server)
                .await;

            let err = provider(&server)
                .fetch_supported_models()
                .await
                .unwrap_err();

            assert!(matches!(err, ProviderError::RequestFailed(_)));
            assert!(err.to_string().contains("endpoints down"));
            assert!(err.to_string().contains("services down"));
        }
    }
}
