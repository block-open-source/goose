use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use rmcp::model::Tool;

use crate::anthropic::{AnthropicProvider, AnthropicProviderBuilder, ANTHROPIC_API_VERSION};
use crate::api_client::{ApiClient, AuthMethod, RequestBuilderDecorator, TlsConfig};
use crate::base::{
    ConfigKey, MessageStream, ModelInfo, Provider, ProviderDescriptor, ProviderMetadata,
};
use crate::conversation::message::Message;
use crate::errors::ProviderError;
use crate::formats::openai::{extract_reasoning_effort, is_openai_responses_model};
use crate::model::ModelConfig;
use crate::openai::{OpenAiProvider, OpenAiProviderBuilder};
use crate::openai_compatible::{handle_response_openai_compat, OpenAiCompatibleProvider};

pub const AZURE_FOUNDRY_PROVIDER_NAME: &str = "azure_foundry";
pub const AZURE_FOUNDRY_DEFAULT_MODEL: &str = "Phi-4";
pub const AZURE_FOUNDRY_DOC_URL: &str =
    "https://learn.microsoft.com/azure/ai-foundry/foundry-models/how-to/inference";

const DEPLOYMENT_METADATA_TIMEOUT_SECS: u64 = 5;
const DEPLOYMENT_METADATA_TTL_SECS: u64 = 60;

pub const AZURE_FOUNDRY_KNOWN_MODELS: &[&str] = &[
    "Phi-4",
    "Phi-4-mini",
    "Meta-Llama-3.3-70B-Instruct",
    "Mistral-large-2411",
    "Cohere-command-r-plus-08-2024",
    "AI21-Jamba-1.5-Large",
    "DeepSeek-R1",
    "DeepSeek-V3",
    "glm-4.7",
    "Kimi-K2-Instruct",
    "claude-sonnet-4-6",
    "claude-opus-4-6",
    "gpt-5",
    "o3",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    Maas,
    Resource,
    Project,
}

pub fn endpoint_kind(endpoint: &str) -> EndpointKind {
    if endpoint.contains("/api/projects/") {
        EndpointKind::Project
    } else if endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint)
        .split('/')
        .next()
        .is_some_and(|host| host.ends_with(".services.ai.azure.com"))
    {
        EndpointKind::Resource
    } else {
        EndpointKind::Maas
    }
}

pub fn is_project_endpoint(endpoint: &str) -> bool {
    endpoint_kind(endpoint) == EndpointKind::Project
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPublisher {
    OpenAi,
    Anthropic,
    Partner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeploymentMetadata {
    publisher: ModelPublisher,
    model_name: String,
}

#[derive(Default)]
struct DeploymentCache {
    deployments: HashMap<String, DeploymentMetadata>,
    fetched_at: Option<Instant>,
    fetch_failed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeploymentMetadataLookup {
    ContextDiscovery,
    InferenceRouting,
}

impl DeploymentCache {
    fn applies_to(&self, lookup: DeploymentMetadataLookup) -> bool {
        self.fetched_at.is_some_and(|fetched_at| {
            fetched_at.elapsed() < Duration::from_secs(DEPLOYMENT_METADATA_TTL_SECS)
        }) && (lookup == DeploymentMetadataLookup::ContextDiscovery || !self.fetch_failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InferenceRoute {
    MaasChatCompletions,
    ProjectChatCompletions,
    ProjectResponses,
    AnthropicMessages,
}

fn inference_route(
    project_endpoint: bool,
    publisher: ModelPublisher,
    underlying_model: &str,
) -> InferenceRoute {
    if !project_endpoint {
        return InferenceRoute::MaasChatCompletions;
    }
    match publisher {
        ModelPublisher::Anthropic => InferenceRoute::AnthropicMessages,
        ModelPublisher::OpenAi if is_openai_responses_model(underlying_model) => {
            InferenceRoute::ProjectResponses
        }
        ModelPublisher::OpenAi | ModelPublisher::Partner => InferenceRoute::ProjectChatCompletions,
    }
}

impl ModelPublisher {
    fn from_azure(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "openai" => Self::OpenAi,
            "anthropic" => Self::Anthropic,
            _ => Self::Partner,
        }
    }

    fn from_model_name(value: &str) -> Self {
        let value = value.to_ascii_lowercase();
        if value.starts_with("claude") {
            Self::Anthropic
        } else if is_openai_responses_model(&value) {
            Self::OpenAi
        } else {
            Self::Partner
        }
    }
}

pub struct AzureFoundryProvider {
    chat: OpenAiCompatibleProvider,
    responses: Option<OpenAiProvider>,
    anthropic: Option<AnthropicProvider>,
    deployments_client: ApiClient,
    endpoint: String,
    api_version: Option<String>,
    maas_model: Option<String>,
    deployments: Mutex<DeploymentCache>,
}

impl ProviderDescriptor for AzureFoundryProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            AZURE_FOUNDRY_PROVIDER_NAME,
            "Azure AI Foundry",
            "OpenAI, Anthropic, and partner models deployed through Azure AI Foundry",
            AZURE_FOUNDRY_DEFAULT_MODEL,
            AZURE_FOUNDRY_KNOWN_MODELS.to_vec(),
            AZURE_FOUNDRY_DOC_URL,
            vec![
                ConfigKey::new("AZURE_FOUNDRY_ENDPOINT", true, false, None, true),
                ConfigKey::new("AZURE_FOUNDRY_API_KEY", false, true, Some(""), true),
                ConfigKey::new("AZURE_FOUNDRY_AD_TOKEN", false, true, Some(""), false),
                ConfigKey::new("AZURE_FOUNDRY_MODEL", false, false, None, true),
                ConfigKey::new("AZURE_FOUNDRY_API_VERSION", false, false, None, false),
            ],
        )
    }
}

impl AzureFoundryProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        endpoint: String,
        api_version: Option<String>,
        maas_model: Option<String>,
        chat_auth: AuthMethod,
        responses_auth: AuthMethod,
        anthropic_auth: AuthMethod,
        deployments_auth: AuthMethod,
        tls_config: Option<TlsConfig>,
        request_builder: Option<RequestBuilderDecorator>,
    ) -> Result<Self> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let endpoint_kind = endpoint_kind(&endpoint);
        let native_inference = endpoint_kind != EndpointKind::Maas;
        let maas_model = if native_inference {
            None
        } else {
            Some(
                maas_model
                    .filter(|model| !model.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("AZURE_FOUNDRY_MODEL is required for MaaS endpoints")
                    })?
                    .trim()
                    .to_string(),
            )
        };
        let chat_prefix = if native_inference {
            "openai/v1/"
        } else {
            "v1/"
        };

        let chat_client = configured_client(
            endpoint.clone(),
            chat_auth,
            tls_config.clone(),
            request_builder.clone(),
        )?;
        let chat = OpenAiCompatibleProvider::new(
            AZURE_FOUNDRY_PROVIDER_NAME.to_string(),
            chat_client,
            chat_prefix.to_string(),
        );

        let (responses, anthropic) = if native_inference {
            let responses_client = configured_client(
                endpoint.clone(),
                responses_auth,
                tls_config.clone(),
                request_builder.clone(),
            )?;
            let responses = OpenAiProviderBuilder::new(responses_client)
                .name(AZURE_FOUNDRY_PROVIDER_NAME)
                .base_path("openai/v1/responses")
                .skip_canonical_filtering(true)
                .build();

            let hub = endpoint.split("/api/projects/").next().unwrap_or(&endpoint);
            let anthropic_client = configured_client(
                format!("{hub}/anthropic"),
                anthropic_auth,
                tls_config.clone(),
                request_builder.clone(),
            )?
            .with_header("anthropic-version", ANTHROPIC_API_VERSION)?;
            let anthropic = AnthropicProviderBuilder::new(anthropic_client)
                .name(AZURE_FOUNDRY_PROVIDER_NAME)
                .skip_canonical_filtering(true)
                .build();
            (Some(responses), Some(anthropic))
        } else {
            (None, None)
        };

        let deployments_client = configured_client(
            endpoint.clone(),
            deployments_auth,
            tls_config,
            request_builder,
        )?;

        Ok(Self {
            chat,
            responses,
            anthropic,
            deployments_client,
            endpoint,
            api_version,
            maas_model,
            deployments: Mutex::new(DeploymentCache::default()),
        })
    }

    async fn fetch_deployments(
        &self,
    ) -> Result<(Vec<String>, HashMap<String, DeploymentMetadata>), ProviderError> {
        let version = self
            .api_version
            .as_deref()
            .or_else(|| is_project_endpoint(&self.endpoint).then_some("v1"));
        let mut next = Some(match version {
            Some(version) => format!("deployments?api-version={version}"),
            None => "deployments".to_string(),
        });
        let mut models = Vec::new();
        let mut deployments = HashMap::new();

        while let Some(path) = next {
            let response = self
                .deployments_client
                .response_get(&path)
                .await
                .map_err(|error| ProviderError::NetworkError(error.to_string()))?;
            let json = handle_response_openai_compat(response).await?;
            if let Some(items) = json.get("value").and_then(|value| value.as_array()) {
                for item in items {
                    let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    let model_name = item
                        .get("modelName")
                        .and_then(|value| value.as_str())
                        .unwrap_or(name);
                    let publisher = item
                        .get("modelPublisher")
                        .and_then(|value| value.as_str())
                        .map(ModelPublisher::from_azure)
                        .unwrap_or_else(|| ModelPublisher::from_model_name(model_name));
                    models.push(name.to_string());
                    deployments.insert(
                        name.to_string(),
                        DeploymentMetadata {
                            publisher,
                            model_name: model_name.to_string(),
                        },
                    );
                }
            }
            next = json
                .get("nextLink")
                .and_then(|value| value.as_str())
                .map(|link| with_api_version(link, version));
        }

        models.sort();
        models.dedup();
        Ok((models, deployments))
    }

    async fn deployment_for(
        &self,
        deployment_name: &str,
        lookup: DeploymentMetadataLookup,
    ) -> Option<DeploymentMetadata> {
        {
            let cache = self
                .deployments
                .lock()
                .expect("Azure Foundry deployment cache poisoned");
            if cache.applies_to(lookup) {
                return cache.deployments.get(deployment_name).cloned();
            }
        }

        let fetch_result = tokio::time::timeout(
            Duration::from_secs(DEPLOYMENT_METADATA_TIMEOUT_SECS),
            self.fetch_deployments(),
        )
        .await
        .ok()
        .and_then(Result::ok);
        let fetch_failed = fetch_result.is_none();
        let deployments = fetch_result
            .map(|(_, deployments)| deployments)
            .unwrap_or_default();
        let deployment = deployments.get(deployment_name).cloned();
        *self
            .deployments
            .lock()
            .expect("Azure Foundry deployment cache poisoned") = DeploymentCache {
            deployments,
            fetched_at: Some(Instant::now()),
            fetch_failed,
        };
        deployment
    }
}

fn with_api_version(link: &str, api_version: Option<&str>) -> String {
    let Some(api_version) = api_version else {
        return link.to_string();
    };
    if link.contains("api-version=") {
        return link.to_string();
    }
    let separator = if link.contains('?') { '&' } else { '?' };
    format!("{link}{separator}api-version={api_version}")
}

fn model_info_for_deployment(deployment_name: &str, model_name: &str) -> ModelInfo {
    let canonical = crate::canonical::maybe_get_canonical_model("azure_foundry", model_name)
        .or_else(|| {
            crate::canonical::maybe_get_canonical_model(
                "azure_foundry",
                &model_name.to_ascii_lowercase(),
            )
        })
        .or_else(|| {
            let (base_model, effort) = extract_reasoning_effort(model_name);
            effort.and_then(|_| {
                crate::canonical::maybe_get_canonical_model("azure_foundry", &base_model).or_else(
                    || {
                        crate::canonical::maybe_get_canonical_model(
                            "azure_foundry",
                            &base_model.to_ascii_lowercase(),
                        )
                    },
                )
            })
        });
    ModelInfo {
        name: deployment_name.to_string(),
        resolved_model: Some(model_name.to_string()),
        context_limit: canonical.as_ref().map(|model| model.limit.context),
        input_token_cost: None,
        output_token_cost: None,
        currency: None,
        supports_cache_control: None,
        reasoning: canonical
            .and_then(|model| model.reasoning)
            .unwrap_or_else(|| ModelConfig::new(model_name).is_reasoning_model()),
        thinking_preservation_format: None,
        request_params: None,
    }
}

fn configured_client(
    host: String,
    auth: AuthMethod,
    tls_config: Option<TlsConfig>,
    request_builder: Option<RequestBuilderDecorator>,
) -> Result<ApiClient> {
    let restrict_redirects = matches!(&auth, AuthMethod::ApiKey { .. });
    let mut client = ApiClient::new_with_tls(host, auth, tls_config)?;
    if restrict_redirects {
        client = client.with_same_origin_redirects()?;
    }
    if let Some(request_builder) = request_builder {
        client = client.with_request_builder(request_builder);
    }
    Ok(client)
}

#[async_trait]
impl Provider for AzureFoundryProvider {
    fn get_name(&self) -> &str {
        AZURE_FOUNDRY_PROVIDER_NAME
    }

    fn skip_canonical_filtering(&self) -> bool {
        true
    }

    async fn refresh_credentials(&self) -> Result<(), ProviderError> {
        self.deployments_client
            .refresh_credentials()
            .await
            .map_err(|error| ProviderError::Authentication(error.to_string()))
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        if let Some(model) = &self.maas_model {
            return Ok(vec![model.clone()]);
        }
        if !is_project_endpoint(&self.endpoint) {
            return Ok(AZURE_FOUNDRY_KNOWN_MODELS
                .iter()
                .map(ToString::to_string)
                .collect());
        }
        let (models, deployments) = self.fetch_deployments().await?;
        *self
            .deployments
            .lock()
            .expect("Azure Foundry deployment cache poisoned") = DeploymentCache {
            deployments,
            fetched_at: Some(Instant::now()),
            fetch_failed: false,
        };
        Ok(models)
    }

    async fn fetch_supported_model_info(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        if let Some(model) = &self.maas_model {
            return Ok(vec![model_info_for_deployment(model, model)]);
        }
        if !is_project_endpoint(&self.endpoint) {
            return Ok(AZURE_FOUNDRY_KNOWN_MODELS
                .iter()
                .map(|model| model_info_for_deployment(model, model))
                .collect());
        }
        let (models, deployments) = self.fetch_deployments().await?;
        let model_info = models
            .iter()
            .filter_map(|name| {
                deployments
                    .get(name)
                    .map(|deployment| model_info_for_deployment(name, &deployment.model_name))
            })
            .collect();
        *self
            .deployments
            .lock()
            .expect("Azure Foundry deployment cache poisoned") = DeploymentCache {
            deployments,
            fetched_at: Some(Instant::now()),
            fetch_failed: false,
        };
        Ok(model_info)
    }

    async fn fetch_model_info(&self, model_name: &str) -> Result<ModelInfo, ProviderError> {
        let resolved_model = if let Some(model) = &self.maas_model {
            model.clone()
        } else if is_project_endpoint(&self.endpoint) {
            self.deployment_for(model_name, DeploymentMetadataLookup::ContextDiscovery)
                .await
                .map(|deployment| deployment.model_name)
                .unwrap_or_else(|| model_name.to_string())
        } else {
            model_name.to_string()
        };
        Ok(model_info_for_deployment(model_name, &resolved_model))
    }

    async fn get_context_limit(&self, model: &str, override_limit: Option<usize>) -> usize {
        goose_provider_types::context_limit::ContextLimitResolver::new(self.get_name())
            .resolve(model, override_limit, || async {
                self.fetch_model_info(model)
                    .await
                    .map(|info| info.context_limit)
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
        let maas_config = self.maas_model.as_ref().map(|model| {
            let mut config = model_config.clone();
            config.model_name = model.clone();
            config
        });
        let model_config = maas_config.as_ref().unwrap_or(model_config);
        let wire_model = self
            .maas_model
            .clone()
            .unwrap_or_else(|| model_config.model_name.clone());
        let deployment = if is_project_endpoint(&self.endpoint) {
            self.deployment_for(&wire_model, DeploymentMetadataLookup::InferenceRouting)
                .await
        } else {
            None
        };
        let publisher = deployment
            .as_ref()
            .map(|deployment| deployment.publisher)
            .unwrap_or_else(|| ModelPublisher::from_model_name(&model_config.model_name));
        let underlying_model = deployment
            .as_ref()
            .map(|deployment| deployment.model_name.as_str())
            .unwrap_or(&model_config.model_name);
        let route = inference_route(self.responses.is_some(), publisher, underlying_model);
        let capability_model = deployment
            .as_ref()
            .map(|deployment| deployment.model_name.as_str())
            .unwrap_or(&model_config.model_name);
        let mut capability_config = model_config.clone();
        capability_config.model_name = capability_model.to_string();
        let capability_config =
            capability_config.with_canonical_limits(AZURE_FOUNDRY_PROVIDER_NAME);
        match route {
            InferenceRoute::ProjectResponses => {
                self.responses
                    .as_ref()
                    .expect("checked above")
                    .stream_for_model(
                        &capability_config,
                        &wire_model,
                        capability_model,
                        system,
                        messages,
                        tools,
                    )
                    .await
            }
            InferenceRoute::AnthropicMessages => {
                self.anthropic
                    .as_ref()
                    .expect("checked above")
                    .stream_for_model(&capability_config, &wire_model, system, messages, tools)
                    .await
            }
            InferenceRoute::MaasChatCompletions | InferenceRoute::ProjectChatCompletions => {
                self.chat
                    .stream_for_model(
                        &capability_config,
                        &wire_model,
                        capability_model,
                        system,
                        messages,
                        tools,
                    )
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn project_endpoint(server: &MockServer) -> String {
        format!("{}/api/projects/test", server.uri())
    }

    fn raw_model_config(model_name: &str) -> ModelConfig {
        let mut config = ModelConfig::new(model_name);
        config.model_name = model_name.to_string();
        config
    }

    fn project_provider(server: &MockServer) -> AzureFoundryProvider {
        AzureFoundryProvider::create(
            project_endpoint(server),
            None,
            None,
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            None,
            None,
        )
        .unwrap()
    }

    fn chat_stream() -> String {
        [
            json!({"id":"c1","object":"chat.completion.chunk","model":"test","choices":[{"delta":{"role":"assistant","content":"Hello"},"index":0}]}),
            json!({"id":"c1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}),
            json!({"id":"c1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}),
        ]
        .into_iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .chain(std::iter::once("data: [DONE]\n\n".to_string()))
        .collect()
    }

    fn responses_stream() -> String {
        let created = r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":0,"status":"in_progress","model":"gpt-5","output":[]}}"#;
        let delta = r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"m1","output_index":0,"content_index":0,"delta":"Hello"}"#;
        let completed = r#"data: {"type":"response.completed","sequence_number":3,"response":{"id":"resp_1","object":"response","created_at":0,"status":"completed","model":"gpt-5","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#;
        format!("{created}\n\n{delta}\n\n{completed}\n\ndata: [DONE]\n\n")
    }

    fn anthropic_stream() -> String {
        let start = r#"data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-6","stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}"#;
        let block_start = r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        let delta = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let block_stop = r#"data: {"type":"content_block_stop","index":0}"#;
        let message_delta = r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}"#;
        let stop = r#"data: {"type":"message_stop"}"#;
        format!(
            "{start}\n\n{block_start}\n\n{delta}\n\n{block_stop}\n\n{message_delta}\n\n{stop}\n\n"
        )
    }

    #[test]
    fn routing_retries_cached_deployment_failures() {
        let cache = DeploymentCache {
            fetched_at: Some(Instant::now()),
            fetch_failed: true,
            ..Default::default()
        };

        assert!(cache.applies_to(DeploymentMetadataLookup::ContextDiscovery));
        assert!(!cache.applies_to(DeploymentMetadataLookup::InferenceRouting));
    }

    #[test]
    fn routing_reuses_successful_empty_deployment_inventory() {
        let cache = DeploymentCache {
            fetched_at: Some(Instant::now()),
            fetch_failed: false,
            ..Default::default()
        };

        assert!(cache.applies_to(DeploymentMetadataLookup::ContextDiscovery));
        assert!(cache.applies_to(DeploymentMetadataLookup::InferenceRouting));
    }

    #[test]
    fn routing_matrix_uses_endpoint_publisher_and_underlying_model() {
        use InferenceRoute::*;
        assert_eq!(
            inference_route(false, ModelPublisher::OpenAi, "gpt-5"),
            MaasChatCompletions
        );
        assert_eq!(
            inference_route(false, ModelPublisher::Anthropic, "claude-sonnet-4-6"),
            MaasChatCompletions
        );
        assert_eq!(
            inference_route(true, ModelPublisher::OpenAi, "gpt-5"),
            ProjectResponses
        );
        assert_eq!(
            inference_route(true, ModelPublisher::OpenAi, "o3-mini"),
            ProjectResponses
        );
        assert_eq!(
            inference_route(true, ModelPublisher::OpenAi, "gpt-4o"),
            ProjectChatCompletions
        );
        assert_eq!(
            inference_route(true, ModelPublisher::Partner, "gpt-5"),
            ProjectChatCompletions
        );
        assert_eq!(
            inference_route(true, ModelPublisher::Anthropic, "gpt-5"),
            AnthropicMessages
        );
    }

    #[test]
    fn resource_endpoint_routes_declared_models_to_native_surfaces() {
        let resource = endpoint_kind("https://hub.services.ai.azure.com");
        let native_inference = resource != EndpointKind::Maas;

        assert_eq!(
            inference_route(
                native_inference,
                ModelPublisher::from_model_name("gpt-5.6-sol"),
                "gpt-5.6-sol",
            ),
            InferenceRoute::ProjectResponses
        );
        assert_eq!(
            inference_route(
                native_inference,
                ModelPublisher::from_model_name("claude-sonnet-4-6"),
                "claude-sonnet-4-6",
            ),
            InferenceRoute::AnthropicMessages
        );
        assert_eq!(
            inference_route(
                native_inference,
                ModelPublisher::from_model_name("Mistral-large"),
                "Mistral-large",
            ),
            InferenceRoute::ProjectChatCompletions
        );
    }

    #[test]
    fn model_fallback_only_routes_known_native_families() {
        assert_eq!(
            ModelPublisher::from_model_name("gpt-5"),
            ModelPublisher::OpenAi
        );
        assert_eq!(
            ModelPublisher::from_model_name("o3-mini"),
            ModelPublisher::OpenAi
        );
        assert_eq!(
            ModelPublisher::from_model_name("claude-sonnet-4-6"),
            ModelPublisher::Anthropic
        );
        assert_eq!(
            ModelPublisher::from_model_name("Mistral-large"),
            ModelPublisher::Partner
        );
        assert_eq!(
            ModelPublisher::from_model_name("glm-4.7"),
            ModelPublisher::Partner
        );
        assert_eq!(
            ModelPublisher::from_model_name("Kimi-K2"),
            ModelPublisher::Partner
        );
    }

    #[test]
    fn endpoint_type_is_detected() {
        assert_eq!(
            endpoint_kind("https://hub.services.ai.azure.com/api/projects/project"),
            EndpointKind::Project
        );
        assert_eq!(
            endpoint_kind("https://hub.services.ai.azure.com"),
            EndpointKind::Resource
        );
        assert_eq!(
            endpoint_kind("https://deployment.eastus.models.ai.azure.com"),
            EndpointKind::Maas
        );
        assert!(is_project_endpoint(
            "https://hub.services.ai.azure.com/api/projects/project"
        ));
        assert!(!is_project_endpoint("https://hub.services.ai.azure.com"));
    }

    #[test]
    fn pagination_link_keeps_api_version() {
        assert_eq!(
            with_api_version("https://example.test/deployments?page=2", Some("v1")),
            "https://example.test/deployments?page=2&api-version=v1"
        );
        assert_eq!(
            with_api_version(
                "https://example.test/deployments?page=2&api-version=2025-05-01",
                Some("v1")
            ),
            "https://example.test/deployments?page=2&api-version=2025-05-01"
        );
    }

    #[tokio::test]
    async fn foundry_api_keys_do_not_follow_cross_origin_redirects() {
        for header_name in ["api-key", "x-api-key"] {
            let destination = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/capture"))
                .respond_with(ResponseTemplate::new(200))
                .mount(&destination)
                .await;

            let source = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/redirect"))
                .and(header(header_name, "foundry-secret"))
                .respond_with(
                    ResponseTemplate::new(307)
                        .append_header("location", format!("{}/capture", destination.uri())),
                )
                .expect(1)
                .mount(&source)
                .await;

            let client = configured_client(
                source.uri(),
                AuthMethod::ApiKey {
                    header_name: header_name.to_string(),
                    key: "foundry-secret".to_string(),
                },
                None,
                None,
            )
            .unwrap();

            assert!(client.response_get("redirect").await.is_err());
            assert!(destination.received_requests().await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn foundry_api_keys_follow_same_origin_redirects() {
        for header_name in ["api-key", "x-api-key"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/redirect"))
                .respond_with(ResponseTemplate::new(307).append_header("location", "/final"))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/final"))
                .and(header(header_name, "foundry-secret"))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;

            let client = configured_client(
                server.uri(),
                AuthMethod::ApiKey {
                    header_name: header_name.to_string(),
                    key: "foundry-secret".to_string(),
                },
                None,
                None,
            )
            .unwrap();

            assert!(client
                .response_get("redirect")
                .await
                .unwrap()
                .status()
                .is_success());
        }
    }

    #[tokio::test]
    async fn foundry_bearer_redirects_keep_reqwest_authorization_behavior() {
        let destination = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/capture"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&destination)
            .await;

        let source = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .and(header("authorization", "Bearer foundry-token"))
            .respond_with(
                ResponseTemplate::new(307)
                    .append_header("location", format!("{}/capture", destination.uri())),
            )
            .expect(1)
            .mount(&source)
            .await;

        let client = configured_client(
            source.uri(),
            AuthMethod::BearerToken("foundry-token".to_string()),
            None,
            None,
        )
        .unwrap();

        assert!(client
            .response_get("redirect")
            .await
            .unwrap()
            .status()
            .is_success());
        let requests = destination.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].headers.get("authorization").is_none());
    }

    #[test]
    fn deployment_metadata_enriches_context_without_pricing() {
        let info = model_info_for_deployment("production-chat", "gpt-5");
        assert_eq!(info.name, "production-chat");
        assert_eq!(info.resolved_model.as_deref(), Some("gpt-5"));
        assert_eq!(info.context_limit, Some(400_000));
        assert_eq!(info.input_token_cost, None);
        assert_eq!(info.output_token_cost, None);
    }

    #[test]
    fn gpt_5_6_sol_uses_its_full_context_window() {
        let info = model_info_for_deployment("gpt-5.6-sol", "gpt-5.6-sol");

        assert_eq!(info.context_limit, Some(1_050_000));
        assert!(info.reasoning);
    }

    #[tokio::test]
    async fn deployment_discovery_is_paginated_and_preserves_underlying_model() {
        let server = MockServer::start().await;
        let page_two = format!("{}/api/projects/test/deployments?page=2", server.uri());
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .and(query_param("api-version", "v1"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "partner-prod",
                    "modelName": "Mistral-large",
                    "modelVersion": "1",
                    "modelPublisher": "MistralAI"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .and(query_param("api-version", "v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "openai-prod",
                    "modelName": "gpt-5",
                    "modelVersion": "1",
                    "modelPublisher": "OpenAI"
                }],
                "nextLink": page_two
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        let (names, deployments) = provider.fetch_deployments().await.unwrap();
        assert_eq!(names, vec!["openai-prod", "partner-prod"]);
        assert_eq!(deployments["openai-prod"].model_name, "gpt-5");
        assert_eq!(deployments["openai-prod"].publisher, ModelPublisher::OpenAi);
    }

    #[tokio::test]
    async fn custom_deployment_context_uses_underlying_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "production-chat",
                    "modelName": "gpt-5",
                    "modelVersion": "1",
                    "modelPublisher": "OpenAI"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        assert_eq!(
            provider.get_context_limit("production-chat", None).await,
            400_000
        );
    }

    #[tokio::test]
    async fn canonical_like_alias_context_uses_underlying_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "gpt-5-high",
                    "modelName": "custom-128k-model",
                    "modelPublisher": "OpenAI"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        let config = raw_model_config("gpt-5-high");
        assert_eq!(
            provider.get_context_limit(&config.model_name, None).await,
            128_000
        );
    }

    #[tokio::test]
    async fn caller_override_precedes_deployment_metadata() {
        let server = MockServer::start().await;
        let provider = project_provider(&server);
        let config = raw_model_config("gpt-5-high");

        assert_eq!(
            provider
                .get_context_limit(&config.model_name, Some(64_000))
                .await,
            64_000
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn project_inventory_failure_is_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": {"message": "expired"}
            })))
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        assert!(provider.fetch_supported_models().await.is_err());
        assert!(provider.fetch_supported_model_info().await.is_err());
    }

    #[tokio::test]
    async fn custom_openai_deployment_uses_alias_and_underlying_capabilities() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "production-chat",
                    "modelName": "gpt-5",
                    "modelPublisher": "OpenAI"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/test/openai/v1/responses"))
            .and(body_partial_json(json!({"model": "production-chat"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(responses_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        let config = ModelConfig::new("production-chat").with_temperature(Some(0.7));
        provider
            .complete(&config, "system", &[], &[])
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let payload: serde_json::Value = requests
            .iter()
            .find(|request| request.url.path().ends_with("/responses"))
            .unwrap()
            .body_json()
            .unwrap();
        assert_eq!(payload["model"], "production-chat");
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("capability_model").is_none());
        assert!(config.request_params.is_none());
        assert!(serde_json::to_value(&config)
            .unwrap()
            .get("capability_model")
            .is_none());
    }

    #[tokio::test]
    async fn suffixed_deployment_without_metadata_is_preserved_on_the_wire() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/test/openai/v1/responses"))
            .and(body_partial_json(json!({"model": "gpt-5-high"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(responses_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        assert_eq!(
            provider.get_context_limit("gpt-5-high", None).await,
            400_000
        );
        provider
            .complete(&raw_model_config("gpt-5-high"), "system", &[], &[])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn suffixed_deployment_alias_is_preserved_on_the_wire() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "gpt-5-high",
                    "modelName": "gpt-5",
                    "modelPublisher": "OpenAI"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/test/openai/v1/responses"))
            .and(body_partial_json(json!({"model": "gpt-5-high"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(responses_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        let config = raw_model_config("gpt-5-high")
            .with_thinking_effort(crate::thinking::ThinkingEffort::Off);
        assert_eq!(config.model_name, "gpt-5-high");
        provider
            .complete(&config, "system", &[], &[])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn anthropic_alias_uses_underlying_output_limit_and_preserves_override() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "claude-prod",
                    "modelName": "claude-sonnet-4-6",
                    "modelPublisher": "Anthropic"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/anthropic/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(anthropic_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(2)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        provider
            .complete(&ModelConfig::new("claude-prod"), "system", &[], &[])
            .await
            .unwrap();
        provider
            .complete(
                &ModelConfig::new("claude-prod").with_max_tokens(Some(12_345)),
                "system",
                &[],
                &[],
            )
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let max_tokens: Vec<i64> = requests
            .iter()
            .filter(|request| request.url.path().ends_with("/messages"))
            .map(|request| {
                request.body_json::<serde_json::Value>().unwrap()["max_tokens"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(max_tokens, vec![128_000, 12_345]);
    }

    #[tokio::test]
    async fn canonical_claude_deployment_uses_canonical_output_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [{
                    "type": "ModelDeployment",
                    "name": "claude-sonnet-4-6",
                    "modelName": "claude-sonnet-4-6",
                    "modelPublisher": "Anthropic"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/anthropic/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(anthropic_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        provider
            .complete(&raw_model_config("claude-sonnet-4-6"), "system", &[], &[])
            .await
            .unwrap();

        let request = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|request| request.url.path().ends_with("/messages"))
            .unwrap();
        let payload = request.body_json::<serde_json::Value>().unwrap();
        assert_eq!(payload["max_tokens"], 128_000);
    }

    #[tokio::test]
    async fn maas_uses_v1_chat_completions_path_and_bound_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(chat_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let provider = AzureFoundryProvider::create(
            server.uri(),
            None,
            Some("bound-model".to_string()),
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            AuthMethod::NoAuth,
            None,
            None,
        )
        .unwrap();
        let config = raw_model_config("wrong-model");
        provider
            .complete(&config, "system", &[], &[])
            .await
            .unwrap();
        let request = server.received_requests().await.unwrap().pop().unwrap();
        let payload = request.body_json::<serde_json::Value>().unwrap();
        assert_eq!(payload["model"], "bound-model");
        assert!(payload.get("azure_foundry_deployment").is_none());
        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["bound-model"]
        );
    }

    #[tokio::test]
    async fn custom_deployment_names_route_to_all_three_surfaces() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/projects/test/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "value": [
                    {"type":"ModelDeployment","name":"openai-prod","modelName":"gpt-5","modelVersion":"1","modelPublisher":"OpenAI"},
                    {"type":"ModelDeployment","name":"claude-prod","modelName":"claude-sonnet-4-6","modelVersion":"1","modelPublisher":"Anthropic"},
                    {"type":"ModelDeployment","name":"partner-prod","modelName":"Mistral-large","modelVersion":"1","modelPublisher":"MistralAI"}
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/test/openai/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(responses_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/anthropic/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(anthropic_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/projects/test/openai/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(chat_stream())
                    .append_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = project_provider(&server);
        for deployment in ["openai-prod", "claude-prod", "partner-prod"] {
            provider
                .complete(&ModelConfig::new(deployment), "system", &[], &[])
                .await
                .unwrap();
        }
    }
}
