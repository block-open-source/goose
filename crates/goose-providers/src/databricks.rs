use crate::formats::openai::{
    extract_reasoning_effort, is_openai_responses_model, openai_reasoning_effort_for_thinking,
};
use crate::images::ImageFormat;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::api_client::{ApiClient, AuthMethod, TlsConfig};
use crate::base::{ConfigKey, MessageStream, ModelInfo, Provider, ProviderMetadata};
const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 600;
use crate::conversation::message::Message;
use crate::databricks_auth::{
    DatabricksAuth, DatabricksAuthProvider, DatabricksOauthTokenProvider, DatabricksRefreshHook,
    DatabricksSessionIdProvider, DatabricksTokenResolver,
};
use crate::errors::ProviderError;
use crate::formats::databricks::create_request_for_provider;
pub use crate::formats::databricks::DATABRICKS_PROVIDER_NAME;
use crate::formats::openai_responses::create_responses_request;
use crate::model::ModelConfig;
use crate::openai_compatible::{
    handle_status, map_http_error_to_provider_error, sanitize_url, stream_openai_compat,
    stream_responses_compat,
};
use crate::request_log::{start_log, LoggerHandleExt};
use crate::retry::ProviderRetry;
use crate::retry::{
    RetryConfig, DEFAULT_BACKOFF_MULTIPLIER, DEFAULT_INITIAL_RETRY_INTERVAL_MS,
    DEFAULT_MAX_RETRIES, DEFAULT_MAX_RETRY_INTERVAL_MS,
};
use rmcp::model::Tool;
use serde_json::json;

#[derive(Debug, Clone)]
struct DatabricksEndpointInfo {
    name: String,
    upstream_model_name: Option<String>,
    upstream_model_provider: Option<String>,
    reasoning: Option<bool>,
    supports_responses_api: bool,
}

#[derive(Debug, Clone)]
struct DatabricksUpstreamModel {
    name: String,
    provider: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedDatabricksEndpointInfo {
    info: Option<DatabricksEndpointInfo>,
    fetched_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EndpointMetadataLookup {
    ContextDiscovery,
    InferenceRouting,
}

impl CachedDatabricksEndpointInfo {
    fn applies_to(&self, lookup: EndpointMetadataLookup) -> bool {
        self.fetched_at.elapsed() < Duration::from_secs(DATABRICKS_ENDPOINT_METADATA_TTL_SECS)
            && (lookup == EndpointMetadataLookup::ContextDiscovery || self.info.is_some())
    }
}

const DATABRICKS_ENDPOINT_METADATA_TIMEOUT_SECS: u64 = 5;
const DATABRICKS_ENDPOINT_METADATA_TTL_SECS: u64 = 60;
static DATABRICKS_ENDPOINT_INFO_CACHE: LazyLock<
    Mutex<std::collections::HashMap<String, CachedDatabricksEndpointInfo>>,
> = LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
pub const DATABRICKS_DEFAULT_MODEL: &str = "databricks-claude-sonnet-4";
pub const DATABRICKS_KNOWN_MODELS: &[&str] = &[
    "databricks-claude-sonnet-4-5",
    "databricks-meta-llama-3-3-70b-instruct",
    "databricks-meta-llama-3-1-405b-instruct",
];

pub const DATABRICKS_DOC_URL: &str =
    "https://docs.databricks.com/en/generative-ai/external-models/index.html";

#[derive(serde::Serialize)]
pub struct DatabricksProvider {
    #[serde(skip)]
    api_client: ApiClient,
    #[serde(skip)]
    host: String,
    auth: DatabricksAuth,
    image_format: ImageFormat,
    #[serde(skip)]
    retry_config: RetryConfig,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    token_cache: Arc<Mutex<Option<String>>>,
    #[serde(skip)]
    instance_id: Option<String>,
    #[serde(skip)]
    refresh_hook: Option<DatabricksRefreshHook>,
    #[serde(skip)]
    session_id_provider: Option<DatabricksSessionIdProvider>,
}

impl DatabricksProvider {
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
        instance_id: Option<String>,
        refresh_hook: Option<DatabricksRefreshHook>,
        session_id_provider: Option<DatabricksSessionIdProvider>,
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
            host.clone(),
            auth_method,
            Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS),
            tls_config,
        )?;
        if let Some(request_builder) = request_builder {
            api_client = api_client.with_request_builder(request_builder);
        }

        Ok(Self {
            api_client,
            host,
            auth,
            image_format: ImageFormat::OpenAi,
            retry_config,
            name: DATABRICKS_PROVIDER_NAME.to_string(),
            token_cache,
            instance_id,
            refresh_hook,
            session_id_provider,
        })
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

    fn is_claude_model(model_name: &str) -> bool {
        model_name.to_lowercase().contains("claude")
    }

    fn is_reasoning_capable_model_name(model_name: &str) -> bool {
        Self::is_claude_model(model_name) || is_openai_responses_model(model_name)
    }

    fn uses_responses_api(
        endpoint_info: Option<&DatabricksEndpointInfo>,
        model_names: &[&str],
    ) -> bool {
        match endpoint_info {
            Some(info) => info.supports_responses_api,
            None => model_names
                .iter()
                .any(|name| is_openai_responses_model(name)),
        }
    }

    fn endpoint_model_candidates(value: &Value) -> Vec<DatabricksUpstreamModel> {
        let mut candidates: Vec<DatabricksUpstreamModel> = Vec::new();

        fn get_string_at(value: &Value, path: &[&str]) -> Option<String> {
            path.iter()
                .try_fold(value, |current, key| current.get(*key))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        }

        fn push_candidate(
            name: Option<String>,
            provider: Option<String>,
            candidates: &mut Vec<DatabricksUpstreamModel>,
        ) {
            if let Some(name) = name {
                if !candidates.iter().any(|candidate| candidate.name == name) {
                    candidates.push(DatabricksUpstreamModel { name, provider });
                }
            }
        }

        for config_key in ["config", "pending_config"] {
            let Some(config) = value.get(config_key) else {
                continue;
            };

            for collection_key in ["served_entities", "served_models"] {
                let Some(entities) = config.get(collection_key).and_then(|v| v.as_array()) else {
                    continue;
                };

                for entity in entities {
                    push_candidate(
                        get_string_at(entity, &["external_model", "name"]),
                        get_string_at(entity, &["external_model", "provider"]),
                        &mut candidates,
                    );
                    push_candidate(
                        get_string_at(entity, &["foundation_model", "name"]),
                        get_string_at(entity, &["foundation_model", "provider"]),
                        &mut candidates,
                    );
                    push_candidate(
                        get_string_at(entity, &["entity_name"]),
                        None,
                        &mut candidates,
                    );
                }
            }
        }

        candidates
    }

    fn endpoint_info_from_value(endpoint: &Value) -> Option<DatabricksEndpointInfo> {
        let name = endpoint.get("name")?.as_str()?.to_string();
        let supports_responses_api = Self::endpoint_supports_responses_api(endpoint);
        let upstream_model = Self::endpoint_model_candidates(endpoint)
            .into_iter()
            .find(|candidate| candidate.name != name);
        let upstream_model_name = upstream_model.as_ref().map(|model| model.name.clone());
        let upstream_model_provider = upstream_model.and_then(|model| model.provider);

        let reasoning = upstream_model_name
            .as_deref()
            .map(Self::is_reasoning_capable_model_name)
            .or_else(|| Some(Self::is_reasoning_capable_model_name(&name)));

        Some(DatabricksEndpointInfo {
            name,
            upstream_model_name,
            upstream_model_provider,
            reasoning,
            supports_responses_api,
        })
    }

    fn endpoint_supports_responses_api(endpoint: &Value) -> bool {
        fn value_contains_responses_api(value: &Value) -> bool {
            match value {
                Value::Object(map) => {
                    map.get("api_types")
                        .and_then(|api_types| api_types.as_array())
                        .is_some_and(|api_types| {
                            api_types
                                .iter()
                                .any(|api_type| api_type.as_str() == Some("openai/v1/responses"))
                        })
                        || map.values().any(value_contains_responses_api)
                }
                Value::Array(values) => values.iter().any(value_contains_responses_api),
                _ => false,
            }
        }

        let Some(config) = endpoint.get("config") else {
            return false;
        };

        for collection_key in ["served_entities", "served_models"] {
            let Some(entities) = config.get(collection_key).and_then(|v| v.as_array()) else {
                continue;
            };

            if entities.iter().any(value_contains_responses_api) {
                return true;
            }
        }

        false
    }

    async fn fetch_endpoint_info(
        &self,
        endpoint_name: &str,
    ) -> Result<DatabricksEndpointInfo, ProviderError> {
        let response = self
            .api_client
            .request(&format!(
                "api/2.0/serving-endpoints/{}",
                urlencoding::encode(endpoint_name)
            ))
            .response_get()
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!(
                    "Failed to fetch Databricks endpoint metadata: {}",
                    e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(ProviderError::RequestFailed(format!(
                "Failed to fetch Databricks endpoint metadata: {} {}",
                status, detail
            )));
        }

        let json: Value = response.json().await.map_err(|e| {
            ProviderError::RequestFailed(format!(
                "Failed to parse Databricks endpoint metadata: {}",
                e
            ))
        })?;

        Self::endpoint_info_from_value(&json).ok_or_else(|| {
            ProviderError::RequestFailed(
                "Unexpected response format from Databricks endpoint metadata".to_string(),
            )
        })
    }

    async fn resolve_endpoint_info(
        &self,
        endpoint_name: &str,
    ) -> Result<DatabricksEndpointInfo, ProviderError> {
        const MAX_MODEL_SERVING_HOPS: usize = 4;

        let original_endpoint_name = endpoint_name.to_string();
        let mut current_endpoint_name = endpoint_name.to_string();
        let mut visited = HashSet::new();
        let mut last_info: Option<DatabricksEndpointInfo> = None;
        let mut first_hop_supports_responses_api: Option<bool> = None;

        for _ in 0..MAX_MODEL_SERVING_HOPS {
            if !visited.insert(current_endpoint_name.clone()) {
                break;
            }

            let info = self.fetch_endpoint_info(&current_endpoint_name).await?;
            let supports_responses_api =
                *first_hop_supports_responses_api.get_or_insert(info.supports_responses_api);
            let next_endpoint_name = match (
                info.upstream_model_provider.as_deref(),
                info.upstream_model_name.as_deref(),
            ) {
                (Some("databricks-model-serving"), Some(next_endpoint_name))
                    if !visited.contains(next_endpoint_name) =>
                {
                    Some(next_endpoint_name.to_string())
                }
                _ => None,
            };

            if let Some(next_endpoint_name) = next_endpoint_name {
                last_info = Some(info);
                current_endpoint_name = next_endpoint_name;
                continue;
            }

            let mut resolved_info = if info.name == original_endpoint_name {
                info
            } else {
                let upstream_model_name = info
                    .upstream_model_name
                    .clone()
                    .or_else(|| Some(info.name.clone()));
                DatabricksEndpointInfo {
                    name: original_endpoint_name,
                    upstream_model_name,
                    upstream_model_provider: info.upstream_model_provider.clone(),
                    reasoning: info.reasoning,
                    supports_responses_api,
                }
            };
            resolved_info.supports_responses_api = supports_responses_api;
            return Ok(resolved_info);
        }

        last_info
            .map(|info| DatabricksEndpointInfo {
                name: original_endpoint_name,
                upstream_model_name: info.upstream_model_name,
                upstream_model_provider: info.upstream_model_provider,
                reasoning: info.reasoning,
                supports_responses_api: first_hop_supports_responses_api.unwrap_or(false),
            })
            .ok_or_else(|| {
                ProviderError::RequestFailed(
                    "Failed to resolve Databricks endpoint metadata".to_string(),
                )
            })
    }

    async fn resolve_endpoint_info_cached(
        &self,
        endpoint_name: &str,
        lookup: EndpointMetadataLookup,
    ) -> Result<DatabricksEndpointInfo, ProviderError> {
        let cache_key = format!("{}:{}", self.host, endpoint_name);
        let cached = DATABRICKS_ENDPOINT_INFO_CACHE
            .lock()
            .unwrap()
            .get(&cache_key)
            .cloned();

        if let Some(cached) = cached {
            if cached.applies_to(lookup) {
                return cached.info.ok_or_else(|| {
                    ProviderError::RequestFailed(
                        "Databricks endpoint metadata is unavailable".to_string(),
                    )
                });
            }
        }

        let info = tokio::time::timeout(
            Duration::from_secs(DATABRICKS_ENDPOINT_METADATA_TIMEOUT_SECS),
            self.resolve_endpoint_info(endpoint_name),
        )
        .await
        .ok()
        .and_then(Result::ok);
        DATABRICKS_ENDPOINT_INFO_CACHE.lock().unwrap().insert(
            cache_key,
            CachedDatabricksEndpointInfo {
                info: info.clone(),
                fetched_at: Instant::now(),
            },
        );
        info.ok_or_else(|| {
            ProviderError::RequestFailed("Databricks endpoint metadata is unavailable".to_string())
        })
    }

    fn model_info_from_endpoint(info: DatabricksEndpointInfo) -> ModelInfo {
        let context_model = info.upstream_model_name.as_deref().unwrap_or(&info.name);
        let context_limit =
            crate::canonical::maybe_get_canonical_model(DATABRICKS_PROVIDER_NAME, context_model)
                .map(|model| model.limit.context);
        let reasoning = info
            .reasoning
            .unwrap_or_else(|| ModelConfig::new(context_model).is_reasoning_model());

        ModelInfo {
            name: info.name,
            resolved_model: info.upstream_model_name,
            context_limit,
            input_token_cost: None,
            output_token_cost: None,
            currency: None,
            supports_cache_control: None,
            reasoning,
            thinking_preservation_format: None,
            request_params: None,
        }
    }

    fn get_endpoint_path(&self, model_name: &str, is_responses_model: bool) -> String {
        if is_responses_model {
            "serving-endpoints/responses".to_string()
        } else {
            let (clean_name, _) = extract_reasoning_effort(model_name);
            format!("serving-endpoints/{}/invocations", clean_name)
        }
    }

    fn build_client_request_id(&self, session_id: &str) -> Option<String> {
        self.instance_id.as_ref().map(|instance_id| {
            json!({
                "sessionId": format!("{}_{}", instance_id, session_id),
            })
            .to_string()
        })
    }
}

impl crate::base::ProviderDescriptor for DatabricksProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            DATABRICKS_PROVIDER_NAME,
            "Databricks",
            "Models on Databricks AI Gateway",
            DATABRICKS_DEFAULT_MODEL,
            DATABRICKS_KNOWN_MODELS.to_vec(),
            DATABRICKS_DOC_URL,
            vec![
                ConfigKey::new("DATABRICKS_HOST", true, false, None, true),
                ConfigKey::new("DATABRICKS_TOKEN", false, true, None, true),
            ],
        )
    }
}

#[async_trait]
impl Provider for DatabricksProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn retry_config(&self) -> RetryConfig {
        self.retry_config.clone()
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        crate::context_limit::ContextLimitResolver::new(self.get_name())
            .resolve(model, override_limit, || async {
                self.fetch_model_info(model)
                    .await
                    .map(|info| info.context_limit)
            })
            .await
    }

    async fn refresh_credentials(&self) -> Result<(), ProviderError> {
        if let Some(refresh_hook) = &self.refresh_hook {
            refresh_hook();
        }
        *self.token_cache.lock().unwrap() = None;
        tracing::info!("Invalidated secrets cache and token cache for credential refresh");
        Ok(())
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let session_id = self
            .session_id_provider
            .as_ref()
            .and_then(|provider| provider())
            .unwrap_or_default();
        let (endpoint_name, _) = extract_reasoning_effort(&model_config.model_name);
        let endpoint_info = self
            .resolve_endpoint_info_cached(&endpoint_name, EndpointMetadataLookup::InferenceRouting)
            .await
            .ok();
        let effective_model_name = endpoint_info
            .as_ref()
            .and_then(|info| info.upstream_model_name.as_deref())
            .unwrap_or(&model_config.model_name);
        let is_responses_model = Self::uses_responses_api(
            endpoint_info.as_ref(),
            &[&model_config.model_name, effective_model_name],
        );
        let path = if is_responses_model {
            "serving-endpoints/responses".to_string()
        } else {
            self.get_endpoint_path(&model_config.model_name, is_responses_model)
        };
        let client_request_id = self.build_client_request_id(&session_id);

        if is_responses_model {
            let responses_model_config;
            let request_model_config = if effective_model_name != model_config.model_name {
                responses_model_config = {
                    let mut config = model_config.clone();
                    config.model_name = effective_model_name.to_string();
                    config
                };
                &responses_model_config
            } else {
                model_config
            };
            let mut payload =
                create_responses_request(request_model_config, system, messages, tools)?;
            payload["model"] = Value::String(endpoint_name.clone());
            if payload.get("reasoning").is_none() {
                if let Some(effort) = model_config.thinking_effort().and_then(|effort| {
                    openai_reasoning_effort_for_thinking(effective_model_name, effort)
                }) {
                    payload.as_object_mut().unwrap().insert(
                        "reasoning".to_string(),
                        json!({
                            "effort": effort,
                            "summary": "auto",
                        }),
                    );
                }
            }
            payload["stream"] = Value::Bool(true);
            if let Some(ref client_request_id) = client_request_id {
                payload["client_request_id"] = Value::String(client_request_id.clone());
            }

            let mut log = start_log(model_config, &payload)?;

            let response = self
                .with_retry(|| async {
                    let payload_clone = payload.clone();
                    let resp = self
                        .api_client
                        .request(&path)
                        .model_headers(model_config)?
                        .streaming(true)
                        .response_post(&payload_clone)
                        .await?;
                    handle_status(resp).await
                })
                .await
                .inspect_err(|e| {
                    let _ = log.error(e);
                })?;

            stream_responses_compat(response, log)
        } else {
            let format_model_config;
            let request_model_config = if Self::is_claude_model(effective_model_name)
                && !Self::is_claude_model(&model_config.model_name)
            {
                format_model_config = {
                    let mut config = model_config.clone();
                    config.model_name = effective_model_name.to_string();
                    config
                };
                &format_model_config
            } else {
                model_config
            };

            let mut payload = create_request_for_provider(
                DATABRICKS_PROVIDER_NAME,
                request_model_config,
                system,
                messages,
                tools,
                &self.image_format,
            )?;
            payload
                .as_object_mut()
                .expect("payload should have model key")
                .remove("model");
            if let Some(client_request_id) = client_request_id {
                payload["client_request_id"] = Value::String(client_request_id);
            }

            payload
                .as_object_mut()
                .unwrap()
                .insert("stream".to_string(), Value::Bool(true));

            if let Some(opts) = payload
                .get_mut("stream_options")
                .and_then(|v| v.as_object_mut())
            {
                opts.entry("include_usage").or_insert(json!(true));
            } else {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("stream_options".to_string(), json!({"include_usage": true}));
            }

            let mut log = start_log(model_config, &payload)?;
            let response = self
                .with_retry(|| async {
                    let resp = self
                        .api_client
                        .request(&path)
                        .model_headers(model_config)?
                        .streaming(true)
                        .response_post(&payload)
                        .await?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let url = sanitize_url(resp.url().as_str());
                        let error_text = crate::http_status::read_error_body(resp)
                            .await
                            .unwrap_or_default();

                        let json_payload = serde_json::from_str::<Value>(&error_text).ok();
                        return Err(map_http_error_to_provider_error(status, json_payload, &url));
                    }
                    Ok(resp)
                })
                .await;

            let response = match response {
                Err(e) if e.to_string().contains("stream_options") => {
                    payload.as_object_mut().unwrap().remove("stream_options");
                    self.with_retry(|| async {
                        let resp = self
                            .api_client
                            .request(&path)
                            .model_headers(model_config)?
                            .streaming(true)
                            .response_post(&payload)
                            .await?;
                        if !resp.status().is_success() {
                            let status = resp.status();
                            let url = sanitize_url(resp.url().as_str());
                            let error_text = crate::http_status::read_error_body(resp)
                                .await
                                .unwrap_or_default();
                            let json_payload = serde_json::from_str::<Value>(&error_text).ok();
                            return Err(map_http_error_to_provider_error(
                                status,
                                json_payload,
                                &url,
                            ));
                        }
                        Ok(resp)
                    })
                    .await
                    .inspect_err(|e| {
                        let _ = log.error(e);
                    })?
                }
                Err(e) => {
                    let _ = log.error(&e);
                    return Err(e);
                }
                Ok(resp) => resp,
            };

            stream_openai_compat(response, log)
        }
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .fetch_supported_model_info()
            .await?
            .into_iter()
            .map(|model| model.name)
            .collect())
    }

    async fn fetch_supported_model_info(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let response = self
            .api_client
            .request("api/2.0/serving-endpoints")
            .response_get()
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to fetch Databricks models: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(ProviderError::RequestFailed(format!(
                "Failed to fetch Databricks models: {} {}",
                status, detail
            )));
        }

        let json: Value = response.json().await.map_err(|e| {
            ProviderError::RequestFailed(format!("Failed to parse Databricks API response: {}", e))
        })?;

        let endpoints = json
            .get("endpoints")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                ProviderError::RequestFailed(
                    "Unexpected response format from Databricks API: missing 'endpoints' array"
                        .to_string(),
                )
            })?;

        let mut models = Vec::new();
        for endpoint in endpoints {
            if let Some(endpoint_info) = Self::endpoint_info_from_value(endpoint) {
                models.push(Self::model_info_from_endpoint(endpoint_info));
            }
        }

        Ok(models)
    }

    async fn fetch_model_info(&self, model_name: &str) -> Result<ModelInfo, ProviderError> {
        let (endpoint_name, _) = extract_reasoning_effort(model_name);
        let endpoint_info = self
            .resolve_endpoint_info_cached(&endpoint_name, EndpointMetadataLookup::ContextDiscovery)
            .await?;
        Ok(Self::model_info_from_endpoint(endpoint_info))
    }

    async fn fetch_recommended_model_info(
        &self,
        _toolshim: bool,
    ) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_supported_model_info().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_retries_cached_metadata_failures() {
        let cached = CachedDatabricksEndpointInfo {
            info: None,
            fetched_at: Instant::now(),
        };

        assert!(cached.applies_to(EndpointMetadataLookup::ContextDiscovery));
        assert!(!cached.applies_to(EndpointMetadataLookup::InferenceRouting));
    }

    #[test]
    fn routing_reuses_cached_metadata_successes() {
        let cached = CachedDatabricksEndpointInfo {
            info: Some(DatabricksEndpointInfo {
                name: "production-chat".to_string(),
                upstream_model_name: Some("gpt-5".to_string()),
                upstream_model_provider: Some("openai".to_string()),
                reasoning: Some(true),
                supports_responses_api: true,
            }),
            fetched_at: Instant::now(),
        };

        assert!(cached.applies_to(EndpointMetadataLookup::ContextDiscovery));
        assert!(cached.applies_to(EndpointMetadataLookup::InferenceRouting));
    }

    #[test]
    fn endpoint_metadata_marks_reasoning_alias_from_external_model() {
        let endpoint = json!({
            "name": "goose",
            "config": {
                "served_entities": [{
                    "name": "current",
                    "external_model": {
                        "name": "claude-opus-4.6",
                        "provider": "anthropic",
                        "task": "llm/v1/chat"
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert_eq!(info.name, "goose");
        assert_eq!(info.upstream_model_name.as_deref(), Some("claude-opus-4.6"));
        assert_eq!(info.reasoning, Some(true));
        assert!(!info.supports_responses_api);

        let model_info = DatabricksProvider::model_info_from_endpoint(info);
        assert_eq!(model_info.name, "goose");
        assert_eq!(
            model_info.resolved_model.as_deref(),
            Some("claude-opus-4.6")
        );
        assert!(model_info.reasoning);
    }

    #[test]
    fn endpoint_metadata_captures_databricks_model_serving_hop() {
        let endpoint = json!({
            "name": "goose",
            "config": {
                "served_entities": [{
                    "external_model": {
                        "name": "databricks-claude-opus-4-6",
                        "provider": "databricks-model-serving",
                        "task": "llm/v1/chat"
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert_eq!(info.name, "goose");
        assert_eq!(
            info.upstream_model_name.as_deref(),
            Some("databricks-claude-opus-4-6")
        );
        assert_eq!(
            info.upstream_model_provider.as_deref(),
            Some("databricks-model-serving")
        );
        assert_eq!(info.reasoning, Some(true));
    }

    #[test]
    fn endpoint_metadata_marks_reasoning_alias_from_pending_gpt_model() {
        let endpoint = json!({
            "name": "goose",
            "pending_config": {
                "served_entities": [{
                    "external_model": {
                        "name": "gpt-5.5",
                        "provider": "openai",
                        "task": "llm/v1/chat"
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert_eq!(info.name, "goose");
        assert_eq!(info.upstream_model_name.as_deref(), Some("gpt-5.5"));
        assert_eq!(info.reasoning, Some(true));
    }

    #[test]
    fn endpoint_metadata_uses_endpoint_name_when_no_upstream_model_exists() {
        let endpoint = json!({
            "name": "goose-gpt-5-5"
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert_eq!(info.name, "goose-gpt-5-5");
        assert_eq!(info.upstream_model_name, None);
        assert_eq!(info.reasoning, Some(true));
        assert!(!info.supports_responses_api);
    }

    #[test]
    fn endpoint_metadata_detects_responses_api_from_foundation_model_api_types() {
        let endpoint = json!({
            "name": "databricks-gpt-5-4",
            "config": {
                "served_entities": [{
                    "name": "databricks-gpt-5-4",
                    "entity_name": "system.ai.databricks-gpt-5-4",
                    "type": "FOUNDATION_MODEL",
                    "foundation_model": {
                        "name": "system.ai.databricks-gpt-5-4",
                        "display_name": "GPT-5.4",
                        "api_types": [
                            "mlflow/v1/chat/completions",
                            "openai/v1/responses",
                            "cursor/v1/chat/completions"
                        ]
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert_eq!(info.name, "databricks-gpt-5-4");
        assert_eq!(
            info.upstream_model_name.as_deref(),
            Some("system.ai.databricks-gpt-5-4")
        );
        assert!(info.supports_responses_api);
    }

    #[test]
    fn endpoint_metadata_detects_responses_api_from_served_models() {
        let endpoint = json!({
            "name": "databricks-gpt-5-4",
            "config": {
                "served_models": [{
                    "foundation_model": {
                        "api_types": ["openai/v1/responses"]
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert!(info.supports_responses_api);
    }

    #[test]
    fn endpoint_metadata_ignores_pending_config_for_responses_routing() {
        let endpoint = json!({
            "name": "databricks-gpt-5-4",
            "config": {
                "served_entities": [{
                    "foundation_model": {
                        "api_types": ["mlflow/v1/chat/completions"]
                    }
                }]
            },
            "pending_config": {
                "served_entities": [{
                    "foundation_model": {
                        "api_types": ["openai/v1/responses"]
                    }
                }]
            }
        });

        let info = DatabricksProvider::endpoint_info_from_value(&endpoint).unwrap();

        assert!(!info.supports_responses_api);
    }

    #[test]
    fn responses_routing_prefers_metadata_over_model_name() {
        let responses_info = DatabricksEndpointInfo {
            name: "custom".into(),
            upstream_model_name: None,
            upstream_model_provider: None,
            reasoning: None,
            supports_responses_api: true,
        };
        assert!(DatabricksProvider::uses_responses_api(
            Some(&responses_info),
            &["databricks-claude-sonnet-4"]
        ));

        let chat_info = DatabricksEndpointInfo {
            supports_responses_api: false,
            ..responses_info
        };
        assert!(!DatabricksProvider::uses_responses_api(
            Some(&chat_info),
            &["gpt-5.4"]
        ));
    }

    #[test]
    fn responses_routing_falls_back_to_model_name_without_metadata() {
        assert!(DatabricksProvider::uses_responses_api(None, &["gpt-5.4"]));
        assert!(DatabricksProvider::uses_responses_api(
            None,
            &["databricks-claude-sonnet-4", "gpt-5.4"]
        ));
        assert!(!DatabricksProvider::uses_responses_api(
            None,
            &["databricks-claude-sonnet-4"]
        ));
    }
}
