use super::api_client::ApiClient;
use super::base::{known_models_from_registry, ConfigKey, ModelInfo, Provider, ProviderMetadata};
use super::retry::ProviderRetry;
use crate::api_client::{AuthMethod, TlsConfig};
use crate::conversation::message::Message;
use crate::conversation::token_usage::{CostSource, ProviderUsage};
use crate::declarative::{DeclarativeProviderConfig, KeyResolver};
use crate::errors::ProviderError;
use crate::formats::openai::is_openai_responses_model;
use crate::formats::openai::{
    create_request_with_options, get_cost, get_usage, is_reserved_request_param_key,
    record_response_metadata, response_to_message, OpenAiFormatOptions,
};
use crate::formats::openai_responses::{
    create_responses_request_for_model, get_responses_usage, responses_api_to_message,
    ResponsesApiResponse,
};
use crate::http_status::read_json_response;
use crate::images::ImageFormat;
use crate::openai_compatible::{
    handle_response_openai_compat, handle_status, stream_openai_compat, stream_responses_compat,
};
use crate::request_log::{start_log, LoggerHandleExt};
use crate::thinking::ThinkingEffort;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::base::{MessageStream, ProviderDescriptor};
use crate::model::ModelConfig;
use rmcp::model::Tool;

pub const OPEN_AI_PROVIDER_NAME: &str = "openai";
pub const OPEN_AI_DEFAULT_BASE_PATH: &str = "v1/chat/completions";
pub const OPEN_AI_VERSIONLESS_BASE_PATH: &str = "chat/completions";
const OPEN_AI_DEFAULT_RESPONSES_PATH: &str = "v1/responses";
const OPEN_AI_DEFAULT_MODELS_PATH: &str = "v1/models";
const N_CTX_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const N_CTX_FAILURE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
enum CachedContextLimit {
    Success(Option<usize>),
    Failure(Instant),
}

impl CachedContextLimit {
    fn value(self) -> Option<Option<usize>> {
        match self {
            Self::Success(limit) => Some(limit),
            Self::Failure(fetched_at) if fetched_at.elapsed() < N_CTX_FAILURE_TTL => Some(None),
            Self::Failure(_) => None,
        }
    }
}
pub const OPEN_AI_DEFAULT_MODEL: &str = "gpt-4o";

pub const OPEN_AI_DOC_URL: &str = "https://platform.openai.com/docs/models";
const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

type OpenAiBaseUrlParts = (String, Vec<(String, String)>, bool);

/// Ensure a base URL has an explicit scheme.
///
/// Users frequently enter hosts like `localhost:1234` without a scheme. The
/// `url` crate parses such input as `scheme="localhost"`, `path="1234"`,
/// silently dropping both the host and the port. When no `://` is present we
/// prepend a sensible scheme (`http://` for local hosts, `https://`
/// otherwise) so the host and port survive parsing.
pub fn ensure_url_scheme(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.contains("://") {
        return trimmed.to_string();
    }

    let host_part = trimmed.split(['/', '?']).next().unwrap_or(trimmed);
    let bare_host = if let Some(rest) = host_part.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        host_part.split(':').next().unwrap_or(host_part)
    };
    let is_local = bare_host == "localhost"
        || bare_host == "127.0.0.1"
        || bare_host == "0.0.0.0"
        || bare_host == "::1";

    let scheme = if is_local { "http" } else { "https" };
    format!("{}://{}", scheme, trimmed)
}

pub fn parse_openai_base_url(raw_url: &str) -> Result<OpenAiBaseUrlParts> {
    let raw_url = ensure_url_scheme(raw_url);
    let raw_url = raw_url.as_str();
    let parsed = url::Url::parse(raw_url)
        .map_err(|e| anyhow::anyhow!("Invalid OPENAI_BASE_URL '{}': {}", raw_url, e))?;

    let authority = parsed[..url::Position::BeforePath].to_string();
    let query_params: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let path = parsed.path().trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return Ok((authority, query_params, true));
    }

    if path == "/v1" {
        return Ok((authority, query_params, true));
    }
    if let Some(prefix) = path.strip_suffix("/v1") {
        return Ok((format!("{}{}", authority, prefix), query_params, true));
    }

    Ok((format!("{}{}", authority, path), query_params, false))
}

#[derive(Debug, serde::Serialize)]
pub struct OpenAiProvider {
    #[serde(skip)]
    api_client: ApiClient,
    base_path: String,
    organization: Option<String>,
    project: Option<String>,
    custom_headers: Option<HashMap<String, String>>,
    supports_streaming: bool,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    preserve_thinking_context: bool,
    #[serde(skip)]
    n_ctx_cache: Arc<Mutex<HashMap<String, CachedContextLimit>>>,
}

/// Builder for [`OpenAiProvider`].
///
/// Exposes every field of the provider so that constructors living outside
/// `openai.rs` (e.g. in `openai_def.rs`) can assemble a provider without
/// needing direct access to the struct's private fields.
pub struct OpenAiProviderBuilder {
    api_client: ApiClient,
    base_path: String,
    organization: Option<String>,
    project: Option<String>,
    custom_headers: Option<HashMap<String, String>>,
    supports_streaming: bool,
    name: String,
    custom_models: Option<Vec<ModelInfo>>,
    dynamic_models: Option<bool>,
    skip_canonical_filtering: bool,
    preserve_thinking_context: bool,
}

impl OpenAiProviderBuilder {
    pub fn new(api_client: ApiClient) -> Self {
        Self {
            api_client,
            base_path: OPEN_AI_DEFAULT_BASE_PATH.to_string(),
            organization: None,
            project: None,
            custom_headers: None,
            supports_streaming: true,
            name: OPEN_AI_PROVIDER_NAME.to_string(),
            custom_models: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            preserve_thinking_context: false,
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

    pub fn base_path(mut self, base_path: impl Into<String>) -> Self {
        self.base_path = base_path.into();
        self
    }

    pub fn organization(mut self, organization: Option<String>) -> Self {
        self.organization = organization;
        self
    }

    pub fn project(mut self, project: Option<String>) -> Self {
        self.project = project;
        self
    }

    pub fn custom_headers(mut self, custom_headers: Option<HashMap<String, String>>) -> Self {
        self.custom_headers = custom_headers;
        self
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

    pub fn preserve_thinking_context(mut self, preserve_thinking_context: bool) -> Self {
        self.preserve_thinking_context = preserve_thinking_context;
        self
    }

    pub fn build(self) -> OpenAiProvider {
        OpenAiProvider {
            api_client: self.api_client,
            base_path: self.base_path,
            organization: self.organization,
            project: self.project,
            custom_headers: self.custom_headers,
            supports_streaming: self.supports_streaming,
            name: self.name,
            custom_models: self.custom_models,
            dynamic_models: self.dynamic_models,
            skip_canonical_filtering: self.skip_canonical_filtering,
            preserve_thinking_context: self.preserve_thinking_context,
            n_ctx_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl OpenAiProvider {
    pub async fn stream_for_model(
        &self,
        model_config: &ModelConfig,
        wire_model: &str,
        capability_model: &str,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let mut payload = create_responses_request_for_model(
            model_config,
            wire_model,
            capability_model,
            system,
            messages,
            tools,
        )?;
        payload["stream"] = serde_json::Value::Bool(self.supports_streaming);
        self.stream_responses_payload(model_config, payload).await
    }

    async fn stream_responses_payload(
        &self,
        model_config: &ModelConfig,
        payload: serde_json::Value,
    ) -> Result<MessageStream, ProviderError> {
        let mut log = start_log(model_config, &payload)?;
        let response = self
            .with_retry(|| async {
                handle_status(
                    self.api_client
                        .request(&Self::map_base_path(
                            &self.base_path,
                            "responses",
                            OPEN_AI_DEFAULT_RESPONSES_PATH,
                        ))
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
            stream_responses_compat(response, log)
        } else {
            let json: serde_json::Value = read_json_response(response).await?;
            let parsed: ResponsesApiResponse =
                serde_json::from_value(json.clone()).map_err(|e| {
                    ProviderError::ExecutionError(format!(
                        "Failed to parse responses API response: {}",
                        e
                    ))
                })?;
            let message = responses_api_to_message(&parsed)?;
            let usage_data = get_responses_usage(&parsed);
            let usage_json = json.get("usage").unwrap_or(&serde_json::Value::Null);
            let mut usage = ProviderUsage::new(model_config.model_name.clone(), usage_data);
            usage.response_id = Some(parsed.id.clone());
            let finish_reason = json
                .pointer("/incomplete_details/reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&parsed.status);
            usage.finish_reasons = Some(vec![finish_reason.to_string()]);
            if let Some(cost) = get_cost(usage_json) {
                usage = usage.with_cost(cost, CostSource::ProviderReported);
            }
            log.write(
                &serde_json::to_value(&message).unwrap_or_default(),
                Some(&usage_data),
            )?;
            Ok(super::base::stream_from_single_message(message, usage))
        }
    }

    #[doc(hidden)]
    pub fn new(api_client: ApiClient) -> Self {
        Self {
            api_client,
            base_path: OPEN_AI_DEFAULT_BASE_PATH.to_string(),
            organization: None,
            project: None,
            custom_headers: None,
            supports_streaming: true,
            name: OPEN_AI_PROVIDER_NAME.to_string(),
            custom_models: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            preserve_thinking_context: false,
            n_ctx_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn normalize_base_path(base_path: &str) -> String {
        if let Some(path) = base_path.strip_prefix('/') {
            format!("/{}", path.trim_end_matches('/'))
        } else {
            base_path.trim_end_matches('/').to_string()
        }
    }

    fn is_chat_completions_path(base_path: &str) -> bool {
        let normalized = Self::normalize_base_path(base_path).to_ascii_lowercase();
        normalized.contains("chat/completions")
    }

    fn is_responses_path(base_path: &str) -> bool {
        let normalized = Self::normalize_base_path(base_path).to_ascii_lowercase();
        normalized.ends_with("responses") || normalized.contains("/responses")
    }

    fn is_responses_model(model_name: &str) -> bool {
        is_openai_responses_model(model_name)
    }

    fn should_use_responses_api(model_name: &str, base_path: &str) -> bool {
        let normalized_base_path = Self::normalize_base_path(base_path);
        // Only the standard "v1/chat/completions" is treated as a default
        // path that defers to model-based routing.  The versionless
        // "chat/completions" (derived from an OPENAI_BASE_URL without /v1)
        // is treated as custom because versionless gateways typically do not
        // support the Responses API.
        let has_custom_base_path = normalized_base_path != OPEN_AI_DEFAULT_BASE_PATH;

        if has_custom_base_path {
            if Self::is_responses_path(&normalized_base_path) {
                return true;
            }
            if Self::is_chat_completions_path(&normalized_base_path) {
                return false;
            }
        }

        Self::is_responses_model(model_name)
    }

    /// Providers known to reject `max_completion_tokens` and require
    /// the legacy `max_tokens` field instead.
    const PROVIDERS_NEEDING_MAX_TOKENS_REMAP: &[&str] = &[
        "cerebras",
        "custom_deepseek",
        "groq",
        "inception",
        "kimi",
        "lmstudio",
        "mistral",
        "moonshot",
        "nearai",
        "ovhcloud",
    ];

    const PROVIDERS_NEEDING_STANDARD_CHAT_PARAMS: &[&str] = &["nearai", "pleumrouter"];

    /// Providers whose reasoning models accept an OpenAI-style
    /// `reasoning_effort` field on chat-completions requests but aren't
    /// matched by [`is_openai_responses_model`] (which only recognises
    /// OpenAI's own `o*`/`gpt-5*` model names). These need the unified
    /// [`ThinkingEffort`] mapped onto the request explicitly.
    const PROVIDERS_NEEDING_REASONING_EFFORT_MAPPING: &[&str] = &["meta"];

    /// Maps the unified thinking effort onto Meta's Muse Spark
    /// `reasoning_effort` levels: `low`, `medium`, `high`, `xhigh`.
    ///
    /// Muse Spark always reasons and has no supported "disable reasoning"
    /// level, so `Off` is clamped to `low` (the lightest level Meta
    /// supports) rather than sent as-is or omitted.
    fn meta_reasoning_effort(effort: ThinkingEffort) -> &'static str {
        match effort {
            ThinkingEffort::Off | ThinkingEffort::Low => "low",
            ThinkingEffort::Medium => "medium",
            ThinkingEffort::High => "high",
            ThinkingEffort::Max => "xhigh",
        }
    }

    fn declared_model(&self, model_name: &str) -> Option<&ModelInfo> {
        self.custom_models
            .as_ref()?
            .iter()
            .find(|m| m.name == model_name)
    }

    fn sanitize_request_for_compat(
        &self,
        mut payload: serde_json::Value,
        model_config: &ModelConfig,
    ) -> serde_json::Value {
        if let Some(obj) = payload.as_object_mut() {
            if Self::PROVIDERS_NEEDING_MAX_TOKENS_REMAP.contains(&self.name.as_str()) {
                if let Some(value) = obj.remove("max_completion_tokens") {
                    obj.entry("max_tokens").or_insert(value);
                }
            }

            if Self::PROVIDERS_NEEDING_STANDARD_CHAT_PARAMS.contains(&self.name.as_str()) {
                let model_name = obj.get("model").and_then(|model| model.as_str());
                if !model_name.is_some_and(Self::is_responses_model) {
                    obj.remove("reasoning_effort");
                }

                if let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
                    for message in messages {
                        if message
                            .get("role")
                            .and_then(|role| role.as_str())
                            .is_some_and(|role| role == "developer")
                        {
                            message["role"] = serde_json::Value::String("system".to_string());
                        }
                    }
                }
            }

            if Self::PROVIDERS_NEEDING_REASONING_EFFORT_MAPPING.contains(&self.name.as_str()) {
                match model_config.thinking_effort() {
                    Some(effort) => {
                        obj.insert(
                            "reasoning_effort".to_string(),
                            json!(Self::meta_reasoning_effort(effort)),
                        );
                    }
                    None => {
                        obj.remove("reasoning_effort");
                    }
                }
            }
        }

        payload
    }

    fn should_use_responses_api_for_provider(&self, model_name: &str) -> bool {
        if Self::PROVIDERS_NEEDING_STANDARD_CHAT_PARAMS.contains(&self.name.as_str()) {
            return false;
        }

        Self::should_use_responses_api(model_name, &self.base_path)
    }

    fn map_base_path(base_path: &str, target: &str, fallback: &str) -> String {
        let normalized = Self::normalize_base_path(base_path);
        if normalized.ends_with(target) || normalized.contains(&format!("/{target}")) {
            return normalized;
        }

        if Self::is_chat_completions_path(&normalized) {
            return normalized.replacen("chat/completions", target, 1);
        }

        if Self::is_responses_path(&normalized) {
            return normalized.replacen("responses", target, 1);
        }

        if normalized.starts_with('/') {
            format!("/{}", fallback.trim_start_matches('/'))
        } else {
            fallback.to_string()
        }
    }

    async fn fetch_models_from_api(&self) -> Result<Vec<String>, ProviderError> {
        let models_path =
            Self::map_base_path(&self.base_path, "models", OPEN_AI_DEFAULT_MODELS_PATH);
        let response = self.api_client.request(&models_path).response_get().await?;

        if response.status() == StatusCode::NOT_FOUND {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::EndpointNotFound(body));
        }

        let response = handle_status(response).await?;

        let body = response.bytes().await.map_err(|e| {
            ProviderError::NetworkError(format!("Failed to read response body: {}", e))
        })?;
        let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            ProviderError::EndpointNotFound(format!("Response body is not valid JSON: {}", e))
        })?;

        if let Some(err_obj) = json.get("error").filter(|error| !error.is_null()) {
            let msg = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(ProviderError::Authentication(msg.to_string()));
        }

        parse_model_ids(&json)
    }

    /// llama.cpp and Ollama expose the actual allocated context window in the
    /// non-standard `meta.n_ctx` field of `/v1/models`. Returns `None` when absent
    /// (e.g. real OpenAI).
    async fn fetch_n_ctx_from_api(&self, model_name: &str) -> Result<Option<usize>, ProviderError> {
        let models_path =
            Self::map_base_path(&self.base_path, "models", OPEN_AI_DEFAULT_MODELS_PATH);
        let response = self.api_client.request(&models_path).response_get().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let json = handle_response_openai_compat(response).await.map_err(|error| {
            if matches!(&error, ProviderError::RequestFailed(message) if message.contains("not valid JSON")) {
                ProviderError::EndpointNotFound(error.to_string())
            } else {
                error
            }
        })?;
        Ok(parse_n_ctx_from_models(&json, model_name))
    }
}

fn parse_model_ids(json: &serde_json::Value) -> Result<Vec<String>, ProviderError> {
    let models = json
        .get("data")
        .and_then(|value| value.as_array())
        .or_else(|| json.as_array())
        .ok_or_else(|| {
            ProviderError::RequestFailed("Missing models array in JSON response".into())
        })?;
    let mut model_ids: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    model_ids.sort();
    Ok(model_ids)
}

/// Extract `meta.n_ctx` for `model_name` from a `/v1/models` response body.
fn parse_n_ctx_from_models(json: &serde_json::Value, model_name: &str) -> Option<usize> {
    let data = json.get("data")?.as_array()?;

    let n_ctx = |entry: &serde_json::Value| -> Option<usize> {
        entry
            .get("meta")?
            .get("n_ctx")?
            .as_u64()
            .map(|v| v as usize)
    };

    if let Some(entry) = data
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(model_name))
    {
        return n_ctx(entry);
    }

    // For single-model servers without --alias, llama.cpp reports the loaded model
    // file path as id rather than the client's alias, so no entry matches above.
    // Fall back to the sole entry's n_ctx.
    match data.as_slice() {
        [only] => n_ctx(only),
        _ => None,
    }
}

impl ProviderDescriptor for OpenAiProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::with_models(
            OPEN_AI_PROVIDER_NAME,
            "OpenAI",
            "GPT-4 and other OpenAI models, including OpenAI compatible ones",
            OPEN_AI_DEFAULT_MODEL,
            known_models_from_registry(OPEN_AI_PROVIDER_NAME),
            OPEN_AI_DOC_URL,
            vec![
                ConfigKey::new("OPENAI_API_KEY", false, true, None, true),
                ConfigKey::new("OPENAI_BASE_URL", false, false, None, false),
                ConfigKey::new(
                    "OPENAI_HOST",
                    true,
                    false,
                    Some("https://api.openai.com"),
                    false,
                ),
                ConfigKey::new(
                    "OPENAI_BASE_PATH",
                    true,
                    false,
                    Some("v1/chat/completions"),
                    false,
                ),
                ConfigKey::new("OPENAI_ORGANIZATION", false, false, None, false),
                ConfigKey::new("OPENAI_PROJECT", false, false, None, false),
                ConfigKey::new("OPENAI_CUSTOM_HEADERS", false, true, None, false),
                ConfigKey::new("OPENAI_TIMEOUT", false, false, Some("600"), false),
            ],
        )
        .with_setup_steps(vec![
            "Go to https://platform.openai.com and sign up or log in",
            "Navigate to API Keys in the left sidebar",
            "Click 'Create new secret key'",
            "Copy the key and paste it above",
        ])
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
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
        let resolver = goose_provider_types::context_limit::ContextLimitResolver::new(&self.name)
            .with_configured_limits(configured_limits);

        resolver
            .resolve(model, override_limit, || async {
                if let Some(cached) = self
                    .n_ctx_cache
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(model).copied())
                    .and_then(CachedContextLimit::value)
                {
                    return Ok(cached);
                }

                let probed = match tokio::time::timeout(
                    N_CTX_PROBE_TIMEOUT,
                    self.fetch_n_ctx_from_api(model),
                )
                .await
                {
                    Ok(Ok(limit)) => Ok(limit),
                    Ok(Err(error)) if error.is_endpoint_not_found() => Ok(None),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(ProviderError::RequestFailed(
                        "Context-limit discovery timed out".into(),
                    )),
                };

                if let Ok(mut cache) = self.n_ctx_cache.lock() {
                    let cached = match probed.as_ref() {
                        Ok(limit) => CachedContextLimit::Success(*limit),
                        Err(_) => CachedContextLimit::Failure(Instant::now()),
                    };
                    cache.insert(model.to_string(), cached);
                }
                probed
            })
            .await
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        if let Some(custom_models) = &self.custom_models {
            let names: Vec<String> = custom_models.iter().map(|m| m.name.clone()).collect();
            if self.dynamic_models == Some(false) {
                return Ok(names);
            }
            match self.fetch_models_from_api().await {
                Ok(models) => return Ok(models),
                Err(e) if e.is_endpoint_not_found() => {
                    tracing::debug!(
                        "Models endpoint not implemented for provider '{}' ({}), using predefined list",
                        self.name,
                        e
                    );
                    return Ok(names);
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
        if self.should_use_responses_api_for_provider(&model_config.model_name) {
            let (wire_model, _) =
                crate::formats::openai::extract_reasoning_effort(&model_config.model_name);
            self.stream_for_model(
                model_config,
                &wire_model,
                &model_config.model_name,
                system,
                messages,
                tools,
            )
            .await
        } else {
            let declared_model = self.declared_model(&model_config.model_name);
            let thinking_preservation_format =
                declared_model.and_then(|m| m.thinking_preservation_format);

            let mut payload = create_request_with_options(
                model_config,
                system,
                messages,
                tools,
                &ImageFormat::OpenAi,
                self.supports_streaming,
                OpenAiFormatOptions {
                    preserve_thinking_context: self.preserve_thinking_context
                        || thinking_preservation_format.is_some(),
                    supports_vision: model_config.supports_vision.unwrap_or_default(),
                    thinking_preservation_format,
                },
            )?;

            if let Some(params) = declared_model.and_then(|m| m.request_params.as_ref()) {
                apply_declared_request_params(&mut payload, params);
            }

            let payload = self.sanitize_request_for_compat(payload, model_config);
            let mut log = start_log(model_config, &payload)?;

            let response = self
                .with_retry(|| async {
                    let resp = self
                        .api_client
                        .request(&self.base_path)
                        .model_headers(model_config)?
                        .streaming(self.supports_streaming)
                        .response_post(&payload)
                        .await?;
                    handle_status(resp).await
                })
                .await
                .inspect_err(|e| {
                    let _ = log.error(e);
                })?;

            if self.supports_streaming {
                stream_openai_compat(response, log)
            } else {
                let json: serde_json::Value = read_json_response(response).await?;

                let message = response_to_message(&json).map_err(|e| {
                    ProviderError::RequestFailed(format!("Failed to parse message: {}", e))
                })?;

                let usage_json = json.get("usage").unwrap_or(&serde_json::Value::Null);
                let usage_data = get_usage(usage_json);
                let mut usage = ProviderUsage::new(model_config.model_name.clone(), usage_data);
                record_response_metadata(&mut usage, &json);
                if let Some(cost) = get_cost(usage_json) {
                    usage = usage.with_cost(cost, CostSource::ProviderReported);
                }

                log.write(
                    &serde_json::to_value(&message).unwrap_or_default(),
                    Some(&usage_data),
                )?;

                Ok(super::base::stream_from_single_message(message, usage))
            }
        }
    }
}

/// Merges a model's declared `request_params` into an already-built payload.
///
/// Reserved keys are skipped so a declaration cannot clobber the streaming setup.
fn apply_declared_request_params(
    payload: &mut serde_json::Value,
    params: &HashMap<String, serde_json::Value>,
) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };

    for (key, value) in params {
        if !is_reserved_request_param_key(key) {
            object.insert(key.clone(), value.clone());
        }
    }
}

pub fn from_declarative_config(
    config: DeclarativeProviderConfig,
    tls_config: Option<TlsConfig>,
    key_resolver: impl KeyResolver,
) -> Result<OpenAiProviderBuilder> {
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

    let normalized_base_url = ensure_url_scheme(&config.base_url);
    let url = url::Url::parse(&normalized_base_url)
        .map_err(|e| anyhow::anyhow!("Invalid base URL '{}': {}", config.base_url, e))?;

    let host = url[..url::Position::BeforePath].to_string();
    let base_path = if let Some(ref explicit_path) = config.base_path {
        explicit_path.trim_start_matches('/').to_string()
    } else {
        derive_base_path(url.path())
    };

    let timeout_secs = config.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);

    let auth = match api_key {
        Some(key) if !key.is_empty() => AuthMethod::BearerToken(key),
        _ => AuthMethod::NoAuth,
    };
    let mut api_client = ApiClient::with_timeout_and_tls(
        host,
        auth,
        std::time::Duration::from_secs(timeout_secs),
        tls_config,
    )?;

    if let Some(query) = url.query() {
        let query_params = url::form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        api_client = api_client.with_query(query_params);
    }

    if let Some(headers) = &config.headers {
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
            let header_value = reqwest::header::HeaderValue::from_str(value)?;
            header_map.insert(header_name, header_value);
        }
        api_client = api_client.with_headers(header_map)?;
    }

    Ok(OpenAiProviderBuilder::new(api_client)
        .base_path(base_path)
        .custom_headers(config.headers)
        .supports_streaming(config.supports_streaming.unwrap_or(true))
        .name(config.name.clone())
        .custom_models(custom_models)
        .dynamic_models(config.dynamic_models)
        .skip_canonical_filtering(config.skip_canonical_filtering)
        .preserve_thinking_context(config.preserves_thinking))
}

pub fn parse_custom_headers(s: String) -> HashMap<String, String> {
    s.split(',')
        .filter_map(|header| {
            let mut parts = header.splitn(2, '=');
            let key = parts.next().map(|s| s.trim().to_string())?;
            let value = parts.next().map(|s| s.trim().to_string())?;
            Some((key, value))
        })
        .collect()
}

pub fn derive_base_path(url_path: &str) -> String {
    let stripped = url_path.trim_start_matches('/');
    let normalized = stripped.trim_end_matches('/');
    if normalized.is_empty() {
        "v1/chat/completions".to_string()
    } else if normalized.ends_with("chat/completions") {
        stripped.to_string()
    } else if ends_with_version_segment(normalized) {
        format!("{}/chat/completions", normalized)
    } else {
        format!("{}/v1/chat/completions", normalized)
    }
}

fn ends_with_version_segment(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or(path);
    last.strip_prefix('v')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_client::AuthMethod;
    use serde_json::json;

    fn make_provider(name: &str) -> OpenAiProvider {
        OpenAiProvider {
            api_client: ApiClient::new_with_tls(
                "http://localhost".to_string(),
                AuthMethod::NoAuth,
                None,
            )
            .unwrap(),
            base_path: "v1/chat/completions".to_string(),
            organization: None,
            project: None,
            custom_headers: None,
            supports_streaming: true,
            name: name.to_string(),
            custom_models: None,
            dynamic_models: None,
            skip_canonical_filtering: false,
            preserve_thinking_context: false,
            n_ctx_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[test]
    fn sanitize_remaps_max_completion_tokens_for_compat_provider() {
        let provider = make_provider("mistral");
        let payload = json!({
            "model": "mistral-medium-latest",
            "messages": [],
            "max_completion_tokens": 16384
        });

        let result = provider
            .sanitize_request_for_compat(payload, &ModelConfig::new("mistral-medium-latest"));
        let obj = result.as_object().unwrap();

        assert!(!obj.contains_key("max_completion_tokens"));
        assert_eq!(obj.get("max_tokens").unwrap(), &json!(16384));
    }

    #[test]
    fn sanitize_preserves_existing_max_tokens_for_compat_provider() {
        let provider = make_provider("mistral");
        let payload = json!({
            "model": "mistral-medium-latest",
            "messages": [],
            "max_tokens": 4096,
            "max_completion_tokens": 16384
        });

        let result = provider
            .sanitize_request_for_compat(payload, &ModelConfig::new("mistral-medium-latest"));
        let obj = result.as_object().unwrap();

        assert!(!obj.contains_key("max_completion_tokens"));
        assert_eq!(obj.get("max_tokens").unwrap(), &json!(4096));
    }

    #[test]
    fn sanitize_noop_for_native_openai_provider() {
        let provider = make_provider("openai");
        let payload = json!({
            "model": "o3",
            "messages": [],
            "max_completion_tokens": 16384
        });

        let result = provider.sanitize_request_for_compat(payload, &ModelConfig::new("o3"));
        let obj = result.as_object().unwrap();

        assert!(obj.contains_key("max_completion_tokens"));
        assert!(!obj.contains_key("max_tokens"));
    }

    #[test]
    fn sanitize_noop_for_unknown_provider() {
        let provider = make_provider("some_future_provider");
        let payload = json!({
            "model": "future-model",
            "messages": [],
            "max_completion_tokens": 16384
        });

        let result =
            provider.sanitize_request_for_compat(payload, &ModelConfig::new("future-model"));
        let obj = result.as_object().unwrap();

        assert!(obj.contains_key("max_completion_tokens"));
        assert!(!obj.contains_key("max_tokens"));
    }

    #[test]
    fn sanitize_no_token_params() {
        let provider = make_provider("groq");
        let payload = json!({
            "model": "llama-3.3-70b-versatile",
            "messages": []
        });

        let result = provider.sanitize_request_for_compat(
            payload.clone(),
            &ModelConfig::new("llama-3.3-70b-versatile"),
        );
        assert_eq!(result, payload);
    }

    #[test]
    fn sanitize_nearai_reasoning_chat_params() {
        let provider = make_provider("nearai");
        let payload = json!({
            "model": "Qwen/Qwen3.6-35B-A3B-FP8",
            "messages": [
                {
                    "role": "developer",
                    "content": "system instructions"
                },
                {
                    "role": "user",
                    "content": "hello"
                }
            ],
            "reasoning_effort": "medium",
            "max_completion_tokens": 16384
        });

        let result = provider
            .sanitize_request_for_compat(payload, &ModelConfig::new("Qwen/Qwen3.6-35B-A3B-FP8"));
        let obj = result.as_object().unwrap();

        assert!(!obj.contains_key("reasoning_effort"));
        assert!(!obj.contains_key("max_completion_tokens"));
        assert_eq!(obj.get("max_tokens").unwrap(), &json!(16384));
        assert_eq!(obj["messages"][0]["role"], "system");
        assert_eq!(obj["messages"][1]["role"], "user");
    }

    #[test]
    fn sanitize_nearai_preserves_openai_reasoning_effort() {
        let provider = make_provider("nearai");
        let payload = json!({
            "model": "openai/gpt-5",
            "messages": [],
            "reasoning_effort": "medium",
            "max_completion_tokens": 16384
        });

        let result =
            provider.sanitize_request_for_compat(payload, &ModelConfig::new("openai/gpt-5"));
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("medium")));
        assert!(!obj.contains_key("max_completion_tokens"));
        assert_eq!(obj.get("max_tokens").unwrap(), &json!(16384));
    }

    #[test]
    fn sanitize_meta_applies_reasoning_effort_from_thinking_effort() {
        let provider = make_provider("meta");
        let payload = json!({
            "model": "muse-spark-1.1",
            "messages": []
        });
        let model_config =
            ModelConfig::new("muse-spark-1.1").with_thinking_effort(ThinkingEffort::High);

        let result = provider.sanitize_request_for_compat(payload, &model_config);
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("high")));
    }

    #[test]
    fn sanitize_meta_maps_max_thinking_effort_to_xhigh() {
        let provider = make_provider("meta");
        let payload = json!({
            "model": "muse-spark-1.1",
            "messages": []
        });
        let model_config =
            ModelConfig::new("muse-spark-1.1").with_thinking_effort(ThinkingEffort::Max);

        let result = provider.sanitize_request_for_compat(payload, &model_config);
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("xhigh")));
    }

    #[test]
    fn sanitize_meta_clamps_off_thinking_effort_to_low() {
        // Muse Spark always reasons and has no "disable reasoning" level,
        // so an explicit `Off` must be clamped to the lightest supported
        // level rather than omitted or sent as-is.
        let provider = make_provider("meta");
        let payload = json!({
            "model": "muse-spark-1.1",
            "messages": [],
            "reasoning_effort": "high"
        });
        let model_config =
            ModelConfig::new("muse-spark-1.1").with_thinking_effort(ThinkingEffort::Off);

        let result = provider.sanitize_request_for_compat(payload, &model_config);
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("low")));
    }

    #[test]
    fn sanitize_meta_omits_reasoning_effort_when_unset() {
        let provider = make_provider("meta");
        let payload = json!({
            "model": "muse-spark-1.1",
            "messages": []
        });
        let model_config = ModelConfig::new("muse-spark-1.1");

        let result = provider.sanitize_request_for_compat(payload, &model_config);
        let obj = result.as_object().unwrap();

        assert!(!obj.contains_key("reasoning_effort"));
    }

    #[test]
    fn nearai_uses_chat_completions_for_openai_reasoning_models() {
        let provider = make_provider("nearai");

        assert!(!provider.should_use_responses_api_for_provider("openai/gpt-5"));
        assert!(!provider.should_use_responses_api_for_provider("openai/o3"));
    }

    #[test]
    fn responses_api_routing_uses_model_family_unless_path_forces_chat() {
        for (model_name, base_path, expected) in [
            ("gpt-5.4", "v1/chat/completions", true),
            ("gpt-5.4-xhigh", "v1/chat/completions", true),
            ("gpt-5.6-sol", "v1/chat/completions", true),
            ("gpt-5.6-terra-xhigh", "v1/chat/completions", true),
            ("gpt-5.2-pro-2025-12-11", "v1/chat/completions", true),
            ("gpt-4o", "v1/chat/completions", false),
            ("gpt-5.2-codex", "openai/v1/chat/completions", false),
        ] {
            assert_eq!(
                OpenAiProvider::should_use_responses_api(model_name, base_path),
                expected,
                "unexpected routing for {model_name} via {base_path}"
            );
        }
    }

    #[test]
    fn custom_chat_path_maps_to_responses_path() {
        let responses_path = OpenAiProvider::map_base_path(
            "openai/v1/chat/completions",
            "responses",
            "v1/responses",
        );
        assert_eq!(responses_path, "openai/v1/responses");
    }

    #[test]
    fn responses_path_maps_to_models_path() {
        let models_path =
            OpenAiProvider::map_base_path("openai/v1/responses", "models", "v1/models");
        assert_eq!(models_path, "openai/v1/models");
    }

    #[test]
    fn parse_model_ids_accepts_openai_response() {
        let response = json!({"data": [{"id": "model-b"}, {"id": "model-a"}]});

        assert_eq!(parse_model_ids(&response).unwrap(), ["model-a", "model-b"]);
    }

    #[test]
    fn parse_model_ids_accepts_together_response() {
        let response = json!([
            {"id": "meta-llama/Llama-3.3-70B-Instruct-Turbo", "type": "chat"},
            {"id": "Qwen/Qwen3-Coder-480B-A35B-Instruct-FP8", "type": "code"}
        ]);

        assert_eq!(
            parse_model_ids(&response).unwrap(),
            [
                "Qwen/Qwen3-Coder-480B-A35B-Instruct-FP8",
                "meta-llama/Llama-3.3-70B-Instruct-Turbo"
            ]
        );
    }

    #[test]
    fn parse_model_ids_rejects_unknown_response() {
        let response = json!({"models": []});

        assert!(parse_model_ids(&response).is_err());
    }

    #[test]
    fn unknown_path_falls_back_to_default_models_path() {
        let models_path = OpenAiProvider::map_base_path("custom/path", "models", "v1/models");
        assert_eq!(models_path, "v1/models");
    }

    #[test]
    fn absolute_chat_path_maps_to_absolute_responses_path() {
        let responses_path =
            OpenAiProvider::map_base_path("/v1/chat/completions", "responses", "v1/responses");
        assert_eq!(responses_path, "/v1/responses");
    }

    #[test]
    fn unknown_absolute_path_falls_back_to_absolute_models_path() {
        let models_path = OpenAiProvider::map_base_path("/custom/path", "models", "v1/models");
        assert_eq!(models_path, "/v1/models");
    }
    #[test]
    fn versionless_base_path_opts_out_of_responses_for_codex_models() {
        assert!(!OpenAiProvider::should_use_responses_api(
            "gpt-5-codex",
            "chat/completions"
        ));
    }

    #[test]
    fn ensure_url_scheme_adds_http_for_local_hosts() {
        assert_eq!(ensure_url_scheme("localhost:1234"), "http://localhost:1234");
        assert_eq!(
            ensure_url_scheme("127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080/v1"
        );
        assert_eq!(ensure_url_scheme("0.0.0.0:3000"), "http://0.0.0.0:3000");
        assert_eq!(ensure_url_scheme("[::1]:1234"), "http://[::1]:1234");
    }

    #[test]
    fn ensure_url_scheme_adds_https_for_remote_hosts() {
        assert_eq!(
            ensure_url_scheme("api.example.com:8443/v1"),
            "https://api.example.com:8443/v1"
        );
        assert_eq!(ensure_url_scheme("example.com"), "https://example.com");
    }

    #[test]
    fn ensure_url_scheme_preserves_existing_scheme() {
        assert_eq!(
            ensure_url_scheme("http://localhost:1234"),
            "http://localhost:1234"
        );
        assert_eq!(
            ensure_url_scheme("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
    }

    fn custom_config(base_url: &str) -> DeclarativeProviderConfig {
        DeclarativeProviderConfig {
            name: "test-openai".to_string(),
            engine: crate::declarative::ProviderEngine::OpenAI,
            display_name: "Test OpenAI".to_string(),
            description: None,
            api_key_env: String::new(),
            base_url: base_url.to_string(),
            models: vec![crate::base::ModelInfo::new("test-model").with_context_limit(4096)],
            headers: None,
            session_id_header_override: None,
            timeout_seconds: None,
            supports_streaming: None,
            requires_auth: false,
            catalog_provider_id: None,
            base_path: None,
            env_vars: None,
            auth: None,
            dynamic_models: Some(false),
            skip_canonical_filtering: false,
            model_doc_link: None,
            setup_steps: vec![],
            toolshim: false,
            preserves_thinking: false,
            emit_clear_thinking: false,
            setup: None,
        }
    }

    #[test]
    fn from_custom_config_preserves_ipv6_authority() {
        let provider = from_declarative_config(
            custom_config("http://[::1]:1234/v1"),
            None,
            crate::declarative::EnvKeyResolver,
        )
        .unwrap()
        .build();

        assert_eq!(provider.api_client.host(), "http://[::1]:1234");
    }

    #[test]
    fn from_custom_config_preserves_userinfo_authority() {
        let provider = from_declarative_config(
            custom_config("https://user:pass@gateway.example/v1"),
            None,
            crate::declarative::EnvKeyResolver,
        )
        .unwrap()
        .build();

        assert_eq!(
            provider.api_client.host(),
            "https://user:pass@gateway.example"
        );
    }

    #[test]
    fn parse_n_ctx_falls_back_to_sole_entry_when_id_differs() {
        let body = json!({
            "data": [
                { "id": "/models/qwen3.gguf", "meta": { "n_ctx": 32768 } }
            ]
        });
        assert_eq!(parse_n_ctx_from_models(&body, "qwen3"), Some(32768));
    }

    #[test]
    fn parse_n_ctx_no_fallback_with_multiple_unmatched_entries() {
        let body = json!({
            "data": [
                { "id": "model-a", "meta": { "n_ctx": 4096 } },
                { "id": "model-b", "meta": { "n_ctx": 8192 } }
            ]
        });
        assert_eq!(parse_n_ctx_from_models(&body, "model-c"), None);
    }

    #[test]
    fn derive_base_path_not_removing_api_path() {
        let r = derive_base_path("https://opencode.ai/zen/go");
        assert_eq!(r, "https://opencode.ai/zen/go/v1/chat/completions");
    }

    #[test]
    fn derive_base_path_should_support_v1() {
        let r = derive_base_path("https://opencode.ai/zen/go/v1");
        assert_eq!(r, "https://opencode.ai/zen/go/v1/chat/completions");
    }

    #[test]
    fn derive_base_path_should_support_no_base_path() {
        let r = derive_base_path("https://opencode.ai/");
        assert_eq!(r, "https://opencode.ai/v1/chat/completions");
    }

    #[test]
    fn derive_base_path_preserves_non_v1_version_prefix() {
        let r = derive_base_path("/api/paas/v4");
        assert_eq!(r, "api/paas/v4/chat/completions");
    }

    #[test]
    fn derive_base_path_does_not_treat_v_word_as_version() {
        let r = derive_base_path("/api/voice");
        assert_eq!(r, "api/voice/v1/chat/completions");
    }

    fn make_provider_with_custom_models(
        host: &str,
        base_path: &str,
        custom_models: Vec<String>,
    ) -> OpenAiProvider {
        OpenAiProvider {
            api_client: ApiClient::new_with_tls(host.to_string(), AuthMethod::NoAuth, None)
                .unwrap(),
            base_path: base_path.to_string(),
            organization: None,
            project: None,
            custom_headers: None,
            supports_streaming: true,
            name: "test-provider".to_string(),
            custom_models: Some(
                custom_models
                    .into_iter()
                    .map(|model| ModelInfo::new(model).with_context_limit(4096))
                    .collect(),
            ),
            dynamic_models: Some(true),
            skip_canonical_filtering: false,
            preserve_thinking_context: false,
            n_ctx_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn context_limit_caches_failed_models_probe() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let mut provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["other-model".to_string()],
        );
        provider.custom_models = None;

        for _ in 0..2 {
            assert_eq!(
                provider.get_context_limit("unknown-model", None).await,
                crate::model::DEFAULT_CONTEXT_LIMIT
            );
        }
    }

    #[tokio::test]
    async fn context_limit_retries_expired_models_probe_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let mut provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["other-model".to_string()],
        );
        provider.custom_models = None;
        provider.n_ctx_cache.lock().unwrap().insert(
            "unknown-model".to_string(),
            CachedContextLimit::Failure(Instant::now() - N_CTX_FAILURE_TTL),
        );

        assert_eq!(
            provider.get_context_limit("unknown-model", None).await,
            crate::model::DEFAULT_CONTEXT_LIMIT
        );
    }

    #[tokio::test]
    async fn context_limit_caches_unsupported_models_endpoint() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .expect(1)
            .mount(&server)
            .await;

        let mut provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["other-model".to_string()],
        );
        provider.custom_models = None;

        assert_eq!(
            provider.get_context_limit("unknown-model", None).await,
            crate::model::DEFAULT_CONTEXT_LIMIT
        );
        assert_eq!(
            provider.get_context_limit("unknown-model", None).await,
            crate::model::DEFAULT_CONTEXT_LIMIT
        );
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

        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["static-model".to_string()],
        );

        let err = provider.fetch_models_from_api().await.unwrap_err();
        assert!(err.is_endpoint_not_found(), "got: {:?}", err);
    }

    #[tokio::test]
    async fn fetch_models_returns_request_failed_for_missing_data_field() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .expect(1)
            .mount(&server)
            .await;

        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["static-model".to_string()],
        );

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

        let predefined = vec!["glm-4.5".to_string(), "glm-5".to_string()];
        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            predefined.clone(),
        );

        let models = provider
            .fetch_supported_models()
            .await
            .expect("should fall back");
        assert_eq!(models, predefined);
    }

    #[tokio::test]
    async fn fetch_supported_models_propagates_auth_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "error": {"message": "invalid api key"}
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["static-model".to_string()],
        );

        let err = provider.fetch_supported_models().await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Authentication(_)),
            "got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn fetch_supported_models_does_not_reclassify_400_as_endpoint_not_found() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {"message": "request is not valid JSON"}
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["static-model".to_string()],
        );

        let err = provider.fetch_supported_models().await.unwrap_err();
        assert!(!err.is_endpoint_not_found(), "got: {:?}", err);
    }

    #[tokio::test]
    async fn nonstreaming_chat_accepts_legitimate_response() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })))
            .mount(&server)
            .await;

        let mut provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["test-model".to_string()],
        );
        provider.supports_streaming = false;

        let _stream = provider
            .stream(&ModelConfig::new("test-model"), "", &[], &[])
            .await
            .expect("legitimate non-streaming response should be accepted");
    }

    #[tokio::test]
    async fn nonstreaming_chat_rejects_oversized_response_body() {
        use crate::http_status::MAX_PROVIDER_JSON_RESPONSE_BYTES;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "a".repeat(MAX_PROVIDER_JSON_RESPONSE_BYTES + 1)
                    },
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;

        let mut provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["test-model".to_string()],
        );
        provider.supports_streaming = false;

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

    #[tokio::test]
    async fn fetch_supported_models_accepts_payload_with_extra_fields() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": "model-a"}, {"id": "model-b"}],
                "message": "ok",
                "error": null
            })))
            .mount(&server)
            .await;

        let provider = make_provider_with_custom_models(
            &server.uri(),
            "v1/chat/completions",
            vec!["static-model".to_string()],
        );

        let models = provider.fetch_supported_models().await.unwrap();
        assert_eq!(models, vec!["model-a".to_string(), "model-b".to_string()]);
    }

    use crate::base::ThinkingPreservationFormat;

    fn cerebras_config() -> DeclarativeProviderConfig {
        crate::declarative::fixed_provider_configs()
            .expect("bundled providers should load")
            .into_iter()
            .find(|config| config.name == "cerebras")
            .expect("cerebras should be bundled")
    }

    fn cerebras_provider() -> OpenAiProvider {
        struct StaticKeyResolver;
        impl KeyResolver for StaticKeyResolver {
            type Error = std::convert::Infallible;

            fn resolve_key(&self, _key: &str) -> std::result::Result<String, Self::Error> {
                Ok("test-key".to_string())
            }
        }

        from_declarative_config(cerebras_config(), None, StaticKeyResolver)
            .expect("cerebras config should build a provider")
            .build()
    }

    #[test]
    fn cerebras_models_declare_thinking_preservation_and_reasoning_format() {
        let provider = cerebras_provider();

        for (model, expected_format) in [
            ("gpt-oss-120b", ThinkingPreservationFormat::ContentPrepend),
            ("zai-glm-4.7", ThinkingPreservationFormat::ContentXml),
            ("gemma-4-31b", ThinkingPreservationFormat::ContentPrepend),
        ] {
            let declared = provider
                .declared_model(model)
                .unwrap_or_else(|| panic!("{model} should be declared"));

            assert_eq!(declared.thinking_preservation_format, Some(expected_format));

            let reasoning_format = declared
                .request_params
                .as_ref()
                .and_then(|params| params.get("reasoning_format"));
            assert_eq!(
                reasoning_format,
                Some(&json!("parsed")),
                "{model} must request parsed reasoning"
            );
        }

        assert!(provider.declared_model("not-a-cerebras-model").is_none());
    }

    #[test]
    fn cerebras_preserves_thinking_by_default() {
        assert!(cerebras_config().preserves_thinking);
    }

    #[test]
    fn apply_declared_request_params_skips_reserved_keys() {
        let mut payload = json!({
            "model": "zai-glm-4.7",
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [{"role": "user", "content": "hi"}]
        });

        let params = HashMap::from([
            ("reasoning_format".to_string(), json!("parsed")),
            ("model".to_string(), json!("hijacked")),
            ("stream".to_string(), json!(false)),
            ("stream_options".to_string(), json!(null)),
            ("messages".to_string(), json!([])),
        ]);

        apply_declared_request_params(&mut payload, &params);

        assert_eq!(payload["reasoning_format"], json!("parsed"));
        assert_eq!(payload["model"], json!("zai-glm-4.7"));
        assert_eq!(payload["stream"], json!(true));
        assert_eq!(payload["stream_options"], json!({"include_usage": true}));
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn metadata_comes_from_the_registry() {
        let metadata = OpenAiProvider::metadata();
        let gpt_4o = metadata
            .known_models
            .iter()
            .find(|model| model.name == "gpt-4o")
            .expect("gpt-4o should come from the catalog");
        assert_eq!(gpt_4o.context_limit, Some(128_000));
        assert!(
            metadata
                .known_models
                .iter()
                .any(|model| model.name == "gpt-4"),
            "registry-only list should include models the old hardcoded list had drifted past"
        );
    }
}
