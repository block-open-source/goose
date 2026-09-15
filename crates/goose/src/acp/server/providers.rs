use super::*;
use crate::config::declarative_providers;
use crate::providers::inventory::ensure_refresh_identity_current;
use crate::providers::provider_secrets;
use goose_providers::base::ModelInfo;
use std::str::FromStr;

const ACP_READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn provider_secret_to_dto(secret: provider_secrets::ProviderSecret) -> ProviderSecretDto {
    let storage = match secret.storage {
        provider_secrets::ProviderSecretStorage::SecretStore => {
            ProviderSecretStorageDto::SecretStore
        }
        provider_secrets::ProviderSecretStorage::ProviderCache => {
            ProviderSecretStorageDto::ProviderCache
        }
    };
    let status = match secret.status {
        provider_secrets::ProviderSecretStatus::Valid => ProviderSecretStatusDto::Valid,
        provider_secrets::ProviderSecretStatus::Expired => ProviderSecretStatusDto::Expired,
        provider_secrets::ProviderSecretStatus::Unknown => ProviderSecretStatusDto::Unknown,
    };
    ProviderSecretDto {
        id: secret.id,
        provider: secret.provider,
        provider_display_name: secret.provider_display_name,
        name: secret.name,
        storage,
        expires_at: secret.expires_at.map(|value| value.to_rfc3339()),
        status,
        configured: secret.configured,
        has_secret: secret.has_secret,
        can_delete: secret.can_delete,
        can_configure: secret.can_configure,
        configure_provider: secret.configure_provider,
    }
}

fn inventory_entry_to_dto(entry: ProviderInventoryEntry) -> ProviderInventoryEntryDto {
    let stale = ProviderInventoryService::is_stale(&entry);
    ProviderInventoryEntryDto {
        provider_id: entry.provider_id,
        provider_name: entry.provider_name,
        description: entry.description,
        default_model: entry.default_model,
        configured: entry.configured,
        available: entry.available,
        provider_type: format!("{:?}", entry.provider_type),
        acp: entry.acp,
        visible_in_setup: entry.visible_in_setup,
        deprecated: entry.deprecated,
        replacement: entry.replacement,
        config_keys: entry
            .config_keys
            .into_iter()
            .map(provider_config_key_to_dto)
            .collect(),
        setup_steps: entry.setup_steps,
        supports_refresh: entry.supports_refresh,
        refreshing: entry.refreshing,
        models: entry
            .models
            .into_iter()
            .map(|m| ProviderInventoryModelDto {
                id: m.id,
                name: m.name,
                family: m.family,
                context_limit: m.context_limit,
                reasoning: m.reasoning,
                recommended: m.recommended,
            })
            .collect(),
        last_updated_at: entry.last_updated_at.map(|t| t.to_rfc3339()),
        last_refresh_attempt_at: entry.last_refresh_attempt_at.map(|t| t.to_rfc3339()),
        last_refresh_error: entry.last_refresh_error,
        stale,
    }
}

fn provider_config_key_to_dto(key: crate::providers::base::ConfigKey) -> ProviderConfigKey {
    ProviderConfigKey {
        name: key.name,
        required: key.required,
        secret: key.secret,
        default: key.default,
        oauth_flow: key.oauth_flow,
        device_code_flow: key.device_code_flow,
        primary: key.primary,
    }
}

const SECRET_MASK_PREFIX_LEN: usize = 4;
const SECRET_MASK_FALLBACK: &str = "***";

fn mask_secret_value(value: &str) -> String {
    if value.chars().count() < SECRET_MASK_PREFIX_LEN * 2 {
        return SECRET_MASK_FALLBACK.to_string();
    }

    let prefix: String = value.chars().take(SECRET_MASK_PREFIX_LEN).collect();
    format!("{prefix}...")
}

fn config_value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) if value.is_empty() => None,
        serde_json::Value::String(value) => Some(value.clone()),
        other => serde_json::to_string(other).ok(),
    }
}

fn provider_config_field_value(
    config: &Config,
    key: &crate::providers::base::ConfigKey,
    secrets: Option<&HashMap<String, serde_json::Value>>,
) -> ProviderConfigFieldValueDto {
    let value = if key.secret {
        std::env::var(key.name.to_uppercase()).ok().or_else(|| {
            secrets
                .and_then(|values| values.get(&key.name))
                .and_then(config_value_to_string)
        })
    } else {
        config
            .get_param::<serde_json::Value>(&key.name)
            .ok()
            .and_then(|value| config_value_to_string(&value))
    };

    ProviderConfigFieldValueDto {
        key: key.name.clone(),
        value: value.as_deref().map(|value| {
            if key.secret {
                mask_secret_value(value)
            } else {
                value.to_string()
            }
        }),
        is_set: value.is_some(),
        is_secret: key.secret,
        required: key.required,
    }
}

fn provider_catalog_entry_to_dto(
    entry: crate::providers::catalog::ProviderCatalogEntry,
) -> ProviderTemplateCatalogEntryDto {
    ProviderTemplateCatalogEntryDto {
        provider_id: entry.id,
        name: entry.name,
        format: entry.format,
        api_url: entry.api_url,
        model_count: entry.model_count,
        doc_url: entry.doc_url,
        env_var: entry.env_var,
    }
}

fn provider_setup_category_to_dto(
    category: crate::providers::catalog::ProviderSetupCategory,
) -> ProviderSetupCategoryDto {
    match category {
        crate::providers::catalog::ProviderSetupCategory::Agent => ProviderSetupCategoryDto::Agent,
        crate::providers::catalog::ProviderSetupCategory::Model => ProviderSetupCategoryDto::Model,
    }
}

fn provider_setup_method_to_dto(
    method: crate::providers::catalog::ProviderSetupMethod,
) -> ProviderSetupMethodDto {
    match method {
        crate::providers::catalog::ProviderSetupMethod::None => ProviderSetupMethodDto::None,
        crate::providers::catalog::ProviderSetupMethod::SingleApiKey => {
            ProviderSetupMethodDto::SingleApiKey
        }
        crate::providers::catalog::ProviderSetupMethod::ConfigFields => {
            ProviderSetupMethodDto::ConfigFields
        }
        crate::providers::catalog::ProviderSetupMethod::HostWithOauthFallback => {
            ProviderSetupMethodDto::HostWithOauthFallback
        }
        crate::providers::catalog::ProviderSetupMethod::OauthBrowser => {
            ProviderSetupMethodDto::OauthBrowser
        }
        crate::providers::catalog::ProviderSetupMethod::OauthDeviceCode => {
            ProviderSetupMethodDto::OauthDeviceCode
        }
        crate::providers::catalog::ProviderSetupMethod::CloudCredentials => {
            ProviderSetupMethodDto::CloudCredentials
        }
        crate::providers::catalog::ProviderSetupMethod::Local => ProviderSetupMethodDto::Local,
        crate::providers::catalog::ProviderSetupMethod::CliAuth => ProviderSetupMethodDto::CliAuth,
    }
}

fn provider_setup_group_to_dto(
    group: crate::providers::catalog::ProviderSetupGroup,
) -> ProviderSetupGroupDto {
    match group {
        crate::providers::catalog::ProviderSetupGroup::Default => ProviderSetupGroupDto::Default,
        crate::providers::catalog::ProviderSetupGroup::Additional => {
            ProviderSetupGroupDto::Additional
        }
    }
}

fn provider_setup_entry_to_dto(
    entry: crate::providers::catalog::ProviderSetupCatalogEntry,
) -> ProviderSetupCatalogEntryDto {
    ProviderSetupCatalogEntryDto {
        provider_id: entry.provider_id,
        name: entry.display_name,
        category: provider_setup_category_to_dto(entry.category),
        acp: entry.acp,
        description: entry.description,
        setup_method: provider_setup_method_to_dto(entry.setup_method),
        native_connect_query: entry.native_connect_query,
        fields: entry
            .fields
            .into_iter()
            .map(|field| ProviderSetupFieldDto {
                key: field.key,
                label: field.label,
                secret: field.secret,
                required: field.required,
                placeholder: field.placeholder,
                default_value: field.default_value,
            })
            .collect(),
        binary_name: entry.binary_name,
        doc_url: entry.docs_url,
        group: provider_setup_group_to_dto(entry.group),
        show_only_when_installed: entry.show_only_when_installed,
        aliases: entry.aliases,
        supports_install: entry.setup_capabilities.install,
        supports_auth: entry.setup_capabilities.auth,
        supports_auth_status: entry.setup_capabilities.auth_status,
    }
}

fn provider_template_to_dto(
    template: crate::providers::catalog::ProviderTemplate,
) -> ProviderTemplateDto {
    ProviderTemplateDto {
        provider_id: template.id,
        name: template.name,
        format: template.format,
        api_url: template.api_url,
        models: template
            .models
            .into_iter()
            .map(|model| ProviderTemplateModelDto {
                id: model.id,
                name: model.name,
                context_limit: model.context_limit,
                capabilities: ProviderTemplateCapabilitiesDto {
                    tool_call: model.capabilities.tool_call,
                    reasoning: model.capabilities.reasoning,
                    attachment: model.capabilities.attachment,
                    temperature: model.capabilities.temperature,
                },
                deprecated: model.deprecated,
            })
            .collect(),
        supports_streaming: template.supports_streaming,
        env_var: template.env_var,
        doc_url: template.doc_url,
    }
}

fn custom_provider_engine_to_dto(engine: &declarative_providers::ProviderEngine) -> &'static str {
    match engine {
        declarative_providers::ProviderEngine::OpenAI => "openai_compatible",
        declarative_providers::ProviderEngine::Anthropic => "anthropic_compatible",
        declarative_providers::ProviderEngine::Ollama => "ollama_compatible",
    }
}

fn normalize_custom_provider_engine(engine: &str) -> Result<String, agent_client_protocol::Error> {
    let engine = engine.trim().to_lowercase();
    if declarative_providers::ProviderEngine::from_str(&engine).is_err() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data(format!("Unsupported custom provider engine: {engine}")));
    }

    match engine.as_str() {
        "openai" | "openai_compatible" => Ok("openai_compatible".to_string()),
        "anthropic" | "anthropic_compatible" => Ok("anthropic_compatible".to_string()),
        "ollama" | "ollama_compatible" => Ok("ollama_compatible".to_string()),
        _ => unreachable!("provider engine was validated above"),
    }
}

fn non_empty_trimmed(value: String, field: &str) -> Result<String, agent_client_protocol::Error> {
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(
            agent_client_protocol::Error::invalid_params().data(format!("{field} cannot be empty"))
        );
    }
    Ok(value)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn normalize_custom_provider_upsert(
    mut provider: CustomProviderUpsertDto,
    require_api_key: bool,
) -> Result<CustomProviderUpsertDto, agent_client_protocol::Error> {
    provider.engine = normalize_custom_provider_engine(&provider.engine)?;
    provider.display_name = non_empty_trimmed(provider.display_name, "displayName")?;
    provider.api_url = non_empty_trimmed(provider.api_url, "apiUrl")?;
    let url = url::Url::parse(&provider.api_url).map_err(|_| {
        agent_client_protocol::Error::invalid_params().data("apiUrl must be a valid URL")
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(
            agent_client_protocol::Error::invalid_params().data("apiUrl must use HTTP or HTTPS")
        );
    }

    provider.api_key = provider.api_key.and_then(|api_key| {
        let api_key = api_key.trim().to_string();
        (!api_key.is_empty()).then_some(api_key)
    });
    if require_api_key && provider.requires_auth && provider.api_key.is_none() {
        return Err(agent_client_protocol::Error::invalid_params().data("apiKey cannot be empty"));
    }
    provider.models = provider
        .models
        .into_iter()
        .filter_map(|model| {
            let model = model.trim().to_string();
            (!model.is_empty()).then_some(model)
        })
        .collect();
    if provider.models.is_empty() {
        return Err(agent_client_protocol::Error::invalid_params().data("models cannot be empty"));
    }

    provider.headers = provider
        .headers
        .into_iter()
        .map(|(key, value)| {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if key.is_empty() {
                return Ok(None);
            }
            reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|_| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("Invalid header name: {key}"))
            })?;
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("Invalid header value for: {key}"))
            })?;
            Ok(Some((key, value)))
        })
        .collect::<Result<Vec<_>, agent_client_protocol::Error>>()?
        .into_iter()
        .flatten()
        .collect();
    provider.catalog_provider_id = normalize_optional_string(provider.catalog_provider_id);
    provider.base_path = normalize_optional_string(provider.base_path);
    Ok(provider)
}

fn custom_provider_headers(headers: HashMap<String, String>) -> Option<HashMap<String, String>> {
    (!headers.is_empty()).then_some(headers)
}

fn custom_provider_models(
    names: Vec<String>,
    existing: &[ModelInfo],
    catalog_provider_id: Option<&str>,
) -> Vec<ModelInfo> {
    let catalog_models = catalog_provider_id
        .and_then(crate::providers::catalog::get_provider_template)
        .map(|template| template.models)
        .unwrap_or_default();

    names
        .into_iter()
        .map(|name| {
            existing
                .iter()
                .find(|model| model.name == name)
                .cloned()
                .or_else(|| {
                    catalog_models
                        .iter()
                        .find(|model| model.id == name)
                        .map(|model| ModelInfo::new(&name).with_context_limit(model.context_limit))
                })
                .unwrap_or_else(|| ModelInfo::new(name))
        })
        .collect()
}

fn load_declarative_provider_for_client(
    provider_id: &str,
) -> Result<declarative_providers::LoadedProvider, agent_client_protocol::Error> {
    declarative_providers::load_provider(provider_id).map_err(|error| {
        if error.to_string().contains("Provider not found") {
            agent_client_protocol::Error::invalid_params()
                .data(format!("Unknown provider: {provider_id}"))
        } else if error.to_string().contains("Invalid provider id") {
            agent_client_protocol::Error::invalid_params().data(error.to_string())
        } else {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        }
    })
}

fn custom_provider_config_to_dto(
    config: &declarative_providers::DeclarativeProviderConfig,
) -> CustomProviderConfigDto {
    let api_key_env = normalize_optional_string(Some(config.api_key_env.clone()));
    let api_key_set = api_key_env
        .as_ref()
        .map(|key| {
            Config::global()
                .get_secret::<serde_json::Value>(key)
                .is_ok()
        })
        .unwrap_or(false);

    CustomProviderConfigDto {
        provider_id: config.name.clone(),
        engine: custom_provider_engine_to_dto(&config.engine).to_string(),
        display_name: config.display_name.clone(),
        api_url: config.base_url.clone(),
        models: config
            .models
            .iter()
            .map(|model| model.name.clone())
            .collect(),
        supports_streaming: config.supports_streaming,
        headers: config.headers.clone().unwrap_or_default(),
        requires_auth: config.requires_auth,
        catalog_provider_id: config.catalog_provider_id.clone(),
        base_path: config.base_path.clone(),
        toolshim: config.toolshim,
        api_key_env,
        api_key_set,
        preserves_thinking: config.preserves_thinking,
    }
}

fn refresh_skip_reason_to_dto(reason: RefreshSkipReason) -> RefreshProviderInventorySkipReasonDto {
    match reason {
        RefreshSkipReason::UnknownProvider => {
            RefreshProviderInventorySkipReasonDto::UnknownProvider
        }
        RefreshSkipReason::NotConfigured => RefreshProviderInventorySkipReasonDto::NotConfigured,
        RefreshSkipReason::DoesNotSupportRefresh => {
            RefreshProviderInventorySkipReasonDto::DoesNotSupportRefresh
        }
        RefreshSkipReason::AlreadyRefreshing => {
            RefreshProviderInventorySkipReasonDto::AlreadyRefreshing
        }
    }
}

fn refresh_plan_to_response(refresh_plan: RefreshPlan) -> RefreshProviderInventoryResponse {
    RefreshProviderInventoryResponse {
        started: refresh_plan.started,
        skipped: refresh_plan
            .skipped
            .into_iter()
            .map(|entry| RefreshProviderInventorySkipDto {
                provider_id: entry.provider_id,
                reason: refresh_skip_reason_to_dto(entry.reason),
            })
            .collect(),
    }
}

impl GooseAcpAgent {
    pub(super) async fn on_list_providers(
        &self,
        req: ListProvidersRequest,
    ) -> Result<ListProvidersResponse, agent_client_protocol::Error> {
        let entries = self
            .provider_inventory
            .entries(&req.provider_ids)
            .await
            .internal_err()?;
        Ok(ListProvidersResponse {
            entries: entries.into_iter().map(inventory_entry_to_dto).collect(),
        })
    }

    pub(super) async fn on_list_provider_supported_models(
        &self,
        req: ProviderSupportedModelsListRequest,
    ) -> Result<ProviderSupportedModelsListResponse, agent_client_protocol::Error> {
        let provider = self
            .create_provider(&req.provider_id, Vec::new(), None, true)
            .await
            .internal_err_ctx("Failed to initialize provider")?;
        let models = match provider.fetch_supported_models().await {
            Ok(models) => models,
            Err(goose_providers::errors::ProviderError::Authentication(error)) => {
                return Err(agent_client_protocol::Error::auth_required().data(error));
            }
            Err(goose_providers::errors::ProviderError::NotConfigured) => {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("Provider is not configured: {}", req.provider_id)));
            }
            Err(error) => {
                return Err(agent_client_protocol::Error::internal_error().data(format!(
                    "Failed to fetch provider supported models: {error}"
                )));
            }
        };

        Ok(ProviderSupportedModelsListResponse {
            provider_id: req.provider_id,
            models,
        })
    }

    pub(super) async fn on_check_provider_readiness(
        &self,
        req: ProviderReadinessCheckRequest,
    ) -> Result<ProviderReadinessCheckResponse, agent_client_protocol::Error> {
        let provider = self
            .provider_inventory
            .find_entry_for_provider(&req.provider_id)
            .await;
        if !provider.is_some_and(|entry| entry.acp) {
            return Err(agent_client_protocol::Error::invalid_params().data(format!(
                "Provider is not an ACP provider: {}",
                req.provider_id
            )));
        }

        let result = tokio::time::timeout(
            ACP_READINESS_TIMEOUT,
            self.create_provider(&req.provider_id, Vec::new(), None, true),
        )
        .await;
        let (ready, error) = match result {
            Ok(Ok(provider)) => {
                tokio::task::spawn_blocking(move || drop(provider))
                    .await
                    .internal_err_ctx("Failed to stop provider readiness check")?;
                (true, None)
            }
            Ok(Err(error)) => (false, Some(error.to_string())),
            Err(_) => (
                false,
                Some(format!("Timed out while checking {}", req.provider_id)),
            ),
        };
        Ok(ProviderReadinessCheckResponse {
            provider_id: req.provider_id,
            ready,
            error,
        })
    }

    pub(super) async fn on_list_provider_catalog(
        &self,
        req: ProviderCatalogListRequest,
    ) -> Result<ProviderCatalogListResponse, agent_client_protocol::Error> {
        let formats = match req.format {
            Some(format) => vec![format
                .parse::<crate::providers::catalog::ProviderFormat>()
                .map_err(|error| agent_client_protocol::Error::invalid_params().data(error))?],
            None => vec![
                crate::providers::catalog::ProviderFormat::OpenAI,
                crate::providers::catalog::ProviderFormat::Anthropic,
                crate::providers::catalog::ProviderFormat::Ollama,
            ],
        };

        let mut providers = Vec::new();
        for format in formats {
            providers.extend(
                crate::providers::catalog::get_providers_by_format(format)
                    .await
                    .into_iter()
                    .map(provider_catalog_entry_to_dto),
            );
        }
        providers.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });

        Ok(ProviderCatalogListResponse { providers })
    }

    pub(super) async fn on_list_provider_setup_catalog(
        &self,
        _req: ProviderSetupCatalogListRequest,
    ) -> Result<ProviderSetupCatalogListResponse, agent_client_protocol::Error> {
        let providers = crate::providers::catalog::get_setup_catalog_entries()
            .await
            .into_iter()
            .map(provider_setup_entry_to_dto)
            .collect();
        Ok(ProviderSetupCatalogListResponse { providers })
    }

    pub(super) async fn on_get_provider_catalog_template(
        &self,
        req: ProviderCatalogTemplateRequest,
    ) -> Result<ProviderCatalogTemplateResponse, agent_client_protocol::Error> {
        let template = crate::providers::catalog::get_provider_template(&req.provider_id)
            .ok_or_else(|| {
                agent_client_protocol::Error::invalid_params()
                    .data(format!("Unknown catalog provider: {}", req.provider_id))
            })?;
        Ok(ProviderCatalogTemplateResponse {
            template: provider_template_to_dto(template),
        })
    }

    pub(super) async fn on_create_custom_provider(
        &self,
        req: CustomProviderCreateRequest,
    ) -> Result<CustomProviderCreateResponse, agent_client_protocol::Error> {
        let toolshim = req.toolshim;
        let provider = normalize_custom_provider_upsert(req.provider, true)?;
        let config = declarative_providers::create_custom_provider(
            declarative_providers::CreateCustomProviderParams {
                engine: provider.engine,
                display_name: provider.display_name,
                api_url: provider.api_url,
                api_key: provider.api_key,
                models: custom_provider_models(
                    provider.models,
                    &[],
                    provider.catalog_provider_id.as_deref(),
                ),
                supports_streaming: provider.supports_streaming,
                headers: custom_provider_headers(provider.headers),
                requires_auth: provider.requires_auth,
                catalog_provider_id: provider.catalog_provider_id,
                base_path: provider.base_path,
                toolshim,
                preserves_thinking: provider.preserves_thinking,
                auth: None,
            },
        )
        .internal_err_ctx("Failed to create custom provider")?;

        Config::global().invalidate_secrets_cache();
        crate::providers::refresh_custom_providers()
            .await
            .internal_err_ctx("Failed to refresh custom providers")?;

        let provider_id = config.name;
        let provider_ids = [provider_id.clone()];
        let status = Self::provider_config_status(provider_id.clone()).await;
        let refresh = self.start_provider_inventory_refresh(&provider_ids).await?;
        Ok(CustomProviderCreateResponse {
            provider_id,
            status,
            refresh,
        })
    }

    pub(super) async fn on_read_custom_provider(
        &self,
        req: CustomProviderReadRequest,
    ) -> Result<CustomProviderReadResponse, agent_client_protocol::Error> {
        let loaded = load_declarative_provider_for_client(&req.provider_id)?;
        let status = Self::provider_config_status(req.provider_id).await;
        Ok(CustomProviderReadResponse {
            provider: custom_provider_config_to_dto(&loaded.config),
            editable: loaded.is_editable,
            status,
        })
    }

    pub(super) async fn on_update_custom_provider(
        &self,
        req: CustomProviderUpdateRequest,
    ) -> Result<CustomProviderUpdateResponse, agent_client_protocol::Error> {
        let loaded = load_declarative_provider_for_client(&req.provider_id)?;
        if !loaded.is_editable {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("Provider is not editable: {}", req.provider_id)));
        }

        let provider = normalize_custom_provider_upsert(req.provider, false)?;
        if provider.requires_auth && provider.api_key.is_none() && loaded.config.auth.is_none() {
            let api_key_env = if loaded.config.api_key_env.is_empty() {
                declarative_providers::generate_api_key_name(&req.provider_id)
            } else {
                loaded.config.api_key_env.clone()
            };
            if Config::global().get_secret::<String>(&api_key_env).is_err() {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data("apiKey is required when auth is enabled and no secret is stored"));
            }
        }
        declarative_providers::update_custom_provider(
            declarative_providers::UpdateCustomProviderParams {
                id: req.provider_id.clone(),
                engine: provider.engine,
                display_name: provider.display_name,
                api_url: provider.api_url,
                api_key: if loaded.config.auth.is_some() {
                    None
                } else {
                    provider.api_key
                },
                models: custom_provider_models(
                    provider.models,
                    &loaded.config.models,
                    provider.catalog_provider_id.as_deref(),
                ),
                supports_streaming: provider.supports_streaming,
                headers: Some(provider.headers),
                requires_auth: provider.requires_auth,
                catalog_provider_id: provider.catalog_provider_id,
                base_path: provider.base_path,
                toolshim: req.toolshim,
                preserves_thinking: provider.preserves_thinking,
                // The desktop/ACP form doesn't yet support editing command-based
                // auth, so carry the existing setting forward unchanged rather
                // than silently clearing it — but only while auth stays enabled;
                // disabling auth must actually stop the credential command.
                auth: if provider.requires_auth {
                    loaded.config.auth.clone()
                } else {
                    None
                },
            },
        )
        .internal_err_ctx("Failed to update custom provider")?;

        Config::global().invalidate_secrets_cache();
        crate::providers::refresh_custom_providers()
            .await
            .internal_err_ctx("Failed to refresh custom providers")?;

        let provider_ids = [req.provider_id.clone()];
        let status = Self::provider_config_status(req.provider_id.clone()).await;
        let refresh = self.start_provider_inventory_refresh(&provider_ids).await?;
        Ok(CustomProviderUpdateResponse {
            provider_id: req.provider_id,
            status,
            refresh,
        })
    }

    pub(super) async fn on_delete_custom_provider(
        &self,
        req: CustomProviderDeleteRequest,
    ) -> Result<CustomProviderDeleteResponse, agent_client_protocol::Error> {
        let loaded = load_declarative_provider_for_client(&req.provider_id)?;
        if !loaded.is_editable {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("Provider is not editable: {}", req.provider_id)));
        }

        if Config::global().get_goose_provider().ok().as_deref() == Some(req.provider_id.as_str()) {
            return Err(agent_client_protocol::Error::invalid_params().data(format!(
                "Cannot delete active provider: {}",
                req.provider_id
            )));
        }

        declarative_providers::remove_custom_provider(&req.provider_id)
            .internal_err_ctx("Failed to delete custom provider")?;

        Config::global().invalidate_secrets_cache();
        crate::providers::refresh_custom_providers()
            .await
            .internal_err_ctx("Failed to refresh custom providers")?;

        Ok(CustomProviderDeleteResponse {
            provider_id: req.provider_id,
            refresh: RefreshProviderInventoryResponse {
                started: Vec::new(),
                skipped: Vec::new(),
            },
        })
    }

    pub(super) async fn provider_config_status(provider_id: String) -> ProviderConfigStatusDto {
        let is_configured = match crate::providers::get_from_registry(&provider_id).await {
            Ok(entry) => {
                if entry
                    .metadata()
                    .setup
                    .as_ref()
                    .is_some_and(|setup| setup.acp)
                {
                    return ProviderConfigStatusDto {
                        provider_id: provider_id.clone(),
                        is_configured: crate::config::get_provider_entry(
                            Config::global(),
                            &provider_id,
                        )
                        .is_some_and(|entry| entry.enabled && entry.configured),
                    };
                }
                match tokio::task::spawn_blocking(move || entry.inventory_configured()).await {
                    Ok(is_configured) => is_configured,
                    Err(error) => {
                        warn!(
                            provider = %provider_id,
                            error = %error,
                            "provider config status check failed"
                        );
                        false
                    }
                }
            }
            Err(_) => false,
        };

        ProviderConfigStatusDto {
            provider_id,
            is_configured,
        }
    }

    pub(super) async fn provider_config_statuses(
        provider_ids: &[String],
    ) -> Vec<ProviderConfigStatusDto> {
        let mut ids = if provider_ids.is_empty() {
            crate::providers::providers()
                .await
                .into_iter()
                .map(|(metadata, _)| metadata.name)
                .collect::<Vec<_>>()
        } else {
            provider_ids.to_vec()
        };
        ids.sort();
        ids.dedup();

        let mut statuses = stream::iter(ids)
            .map(Self::provider_config_status)
            .buffer_unordered(PROVIDER_CONFIG_STATUS_CHECK_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        statuses.sort_by(|a, b| a.provider_id.cmp(&b.provider_id));
        statuses
    }

    pub(super) fn spawn_provider_inventory_refresh_jobs(&self, refresh_plan: &RefreshJobPlan) {
        for refresh_job in refresh_plan.started.iter().cloned() {
            let provider_inventory = self.provider_inventory.clone();
            let provider_factory = Arc::clone(&self.provider_factory);
            let provider_id = refresh_job.provider_id.clone();
            let identity = refresh_job.identity.clone();
            let toolshim = refresh_job.toolshim;
            tokio::spawn(async move {
                let mut refresh_guard = provider_inventory.refresh_guard(&identity);
                let provider_result = AssertUnwindSafe(async {
                    provider_factory(provider_id.clone(), Vec::new(), None, true).await
                })
                .catch_unwind()
                .await;

                let fetch_result: Result<Vec<String>> = match provider_result {
                    Ok(Ok(provider)) => {
                        match ensure_refresh_identity_current(&provider_id, &identity).await {
                            Ok(()) => {
                                match AssertUnwindSafe(provider.fetch_recommended_models(toolshim))
                                    .catch_unwind()
                                    .await
                                {
                                    Ok(Ok(models)) => Ok(models),
                                    Ok(Err(error)) => Err(anyhow::anyhow!(error.to_string())),
                                    Err(_) => Err(anyhow::anyhow!(
                                        "provider inventory refresh task panicked"
                                    )),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(anyhow::anyhow!("provider inventory refresh task panicked")),
                };

                match fetch_result {
                    Ok(models) => match provider_inventory
                        .store_refreshed_models_for_identity(&identity, &models)
                        .await
                    {
                        Ok(()) => refresh_guard.complete(),
                        Err(error) => warn!(
                            provider = %provider_id,
                            error = %error,
                            "failed to store refreshed provider inventory"
                        ),
                    },
                    Err(error) => {
                        let error_message = error.to_string();
                        match provider_inventory
                            .store_refresh_error_for_identity(&identity, error_message.clone())
                            .await
                        {
                            Ok(()) => refresh_guard.complete(),
                            Err(store_error) => warn!(
                                provider = %provider_id,
                                error = %store_error,
                                refresh_error = %error_message,
                                "failed to store provider inventory refresh error"
                            ),
                        }
                        warn!(provider = %provider_id, error = %error_message, "provider inventory refresh failed");
                    }
                }
            });
        }
    }

    pub(super) async fn start_provider_inventory_refresh(
        &self,
        provider_ids: &[String],
    ) -> Result<RefreshProviderInventoryResponse, agent_client_protocol::Error> {
        let refresh_job_plan = self
            .provider_inventory
            .plan_refresh_jobs(provider_ids)
            .await
            .internal_err()?;
        self.spawn_provider_inventory_refresh_jobs(&refresh_job_plan);
        Ok(refresh_plan_to_response(
            refresh_job_plan.into_public_plan(),
        ))
    }

    pub(super) async fn on_refresh_provider_inventory(
        &self,
        req: RefreshProviderInventoryRequest,
    ) -> Result<RefreshProviderInventoryResponse, agent_client_protocol::Error> {
        Config::global().invalidate_secrets_cache();
        self.start_provider_inventory_refresh(&req.provider_ids)
            .await
    }

    pub(super) async fn on_read_provider_config(
        &self,
        req: ProviderConfigReadRequest,
    ) -> Result<ProviderConfigReadResponse, agent_client_protocol::Error> {
        let entry = crate::providers::get_from_registry(&req.provider_id)
            .await
            .invalid_params_err_ctx("Unknown provider")?;
        let config = Config::global();
        let config_keys = &entry.metadata().config_keys;
        let secrets = if config_keys.iter().any(|key| key.secret) {
            Some(config.all_secrets().internal_err()?)
        } else {
            None
        };

        Ok(ProviderConfigReadResponse {
            fields: config_keys
                .iter()
                .map(|key| provider_config_field_value(config, key, secrets.as_ref()))
                .collect(),
        })
    }

    pub(super) async fn on_provider_config_status(
        &self,
        req: ProviderConfigStatusRequest,
    ) -> Result<ProviderConfigStatusResponse, agent_client_protocol::Error> {
        Ok(ProviderConfigStatusResponse {
            statuses: Self::provider_config_statuses(&req.provider_ids).await,
        })
    }

    pub(super) async fn on_save_provider_config(
        &self,
        req: ProviderConfigSaveRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        let entry = crate::providers::get_from_registry(&req.provider_id)
            .await
            .invalid_params_err_ctx("Unknown provider")?;
        let metadata = entry.metadata().clone();
        let config = Config::global();
        let mut config_updates = Vec::new();
        let mut secret_updates = Vec::new();

        for field in &req.fields {
            let Some(config_key) = metadata
                .config_keys
                .iter()
                .find(|config_key| config_key.name == field.key)
            else {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("Unsupported provider config field: {}", field.key)));
            };

            let value = field.value.trim();
            if value.is_empty() {
                return Err(agent_client_protocol::Error::invalid_params().data(format!(
                    "Provider config field cannot be empty: {}",
                    field.key
                )));
            }

            if config_key.secret {
                secret_updates.push((
                    config_key.name.clone(),
                    serde_json::Value::String(value.to_string()),
                ));
            } else {
                config_updates.push((config_key.name.clone(), value.to_string()));
            }
        }

        for (key, value) in config_updates {
            config
                .set_param(&key, &value)
                .internal_err_ctx("Failed to save provider config field")?;
        }
        config
            .set_secret_values(&secret_updates)
            .internal_err_ctx("Failed to save provider secret fields")?;

        if metadata.setup.as_ref().is_some_and(|setup| setup.acp) {
            let model = crate::config::get_provider_entry(config, &req.provider_id)
                .map(|entry| entry.model)
                .unwrap_or_else(|| metadata.default_model.clone());
            crate::config::set_provider_entry(
                config,
                &req.provider_id,
                &crate::config::ProviderEntry {
                    enabled: true,
                    model,
                    configured: true,
                },
            )
            .internal_err_ctx("Failed to enable ACP provider")?;
        }

        let provider_ids = [req.provider_id.clone()];
        let status = Self::provider_config_status(req.provider_id.clone()).await;
        let refresh = self.start_provider_inventory_refresh(&provider_ids).await?;
        Ok(ProviderConfigChangeResponse { status, refresh })
    }

    pub(super) async fn on_delete_provider_config(
        &self,
        req: ProviderConfigDeleteRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        let entry = crate::providers::get_from_registry(&req.provider_id)
            .await
            .invalid_params_err_ctx("Unknown provider")?;
        let metadata = entry.metadata().clone();
        let config = Config::global();
        let mut secret_keys = Vec::new();

        for config_key in &metadata.config_keys {
            if config_key.secret {
                secret_keys.push(config_key.name.clone());
            } else {
                config
                    .delete(&config_key.name)
                    .internal_err_ctx("Failed to delete provider config field")?;
            }
        }

        config
            .delete_secret_values(&secret_keys)
            .internal_err_ctx("Failed to delete provider secret fields")?;
        if metadata.setup.as_ref().is_some_and(|setup| setup.acp) {
            if let Some(mut provider) = crate::config::get_provider_entry(config, &req.provider_id)
            {
                provider.enabled = false;
                provider.configured = false;
                crate::config::set_provider_entry(config, &req.provider_id, &provider)
                    .internal_err_ctx("Failed to disable ACP provider")?;
            }
        }
        crate::providers::cleanup_provider(&req.provider_id)
            .await
            .internal_err_ctx("Failed to clean up provider state")?;

        let provider_ids = [req.provider_id.clone()];
        let status = Self::provider_config_status(req.provider_id.clone()).await;
        let refresh = self.start_provider_inventory_refresh(&provider_ids).await?;
        Ok(ProviderConfigChangeResponse { status, refresh })
    }

    pub(super) async fn on_authenticate_provider_config(
        &self,
        req: ProviderConfigAuthenticateRequest,
    ) -> Result<ProviderConfigChangeResponse, agent_client_protocol::Error> {
        let entry = crate::providers::get_from_registry(&req.provider_id)
            .await
            .invalid_params_err_ctx("Unknown provider")?;

        if req.provider_id == crate::providers::huggingface_auth::HUGGINGFACE_PROVIDER_NAME {
            crate::providers::huggingface_auth::configure_oauth()
                .await
                .internal_err_ctx("Failed to authenticate provider")?;
        } else {
            let metadata = entry.metadata().clone();
            if !metadata.config_keys.iter().any(|key| key.oauth_flow) {
                return Err(agent_client_protocol::Error::invalid_params().data(format!(
                    "Provider does not support native authentication: {}",
                    req.provider_id
                )));
            }

            let provider = entry
                .create_with_default_model(Vec::new())
                .await
                .internal_err_ctx("Failed to initialize provider")?;

            if self.supports_goose_custom_notifications() {
                let client_cx = self.client_cx.get().cloned();
                let provider_id = req.provider_id.clone();
                let announce: Box<dyn Fn(String, String, u64) + Send + Sync> =
                    Box::new(move |user_code, verification_uri, expires_in| {
                        let _ = arboard::Clipboard::new()
                            .ok()
                            .and_then(|mut cb| cb.set_text(&user_code).ok());
                        if let Err(e) = webbrowser::open(&verification_uri) {
                            tracing::warn!("Failed to open browser: {}", e);
                        }
                        if let Some(ref cx) = client_cx {
                            let notification = ProviderDeviceCodeNotification {
                                provider_id: provider_id.clone(),
                                user_code,
                                verification_uri,
                                expires_in,
                            };
                            if let Err(e) = cx.send_notification(notification) {
                                tracing::warn!("Failed to send device code notification: {}", e);
                            }
                        }
                    });
                crate::providers::oauth_device_flow::with_device_code_announce(
                    announce,
                    provider.configure_oauth(),
                )
                .await
                .internal_err_ctx("Failed to authenticate provider")?;
            } else {
                provider
                    .configure_oauth()
                    .await
                    .internal_err_ctx("Failed to authenticate provider")?;
            }
        }
        Config::global().invalidate_secrets_cache();

        let provider_ids = [req.provider_id.clone()];
        let status = Self::provider_config_status(req.provider_id.clone()).await;
        let refresh = self.start_provider_inventory_refresh(&provider_ids).await?;
        Ok(ProviderConfigChangeResponse { status, refresh })
    }

    pub(super) async fn on_list_provider_secrets(
        &self,
        _req: ProviderSecretsListRequest,
    ) -> Result<ProviderSecretsListResponse, agent_client_protocol::Error> {
        let secrets = provider_secrets::list_provider_secrets()
            .await
            .internal_err_ctx("Failed to list provider secrets")?
            .into_iter()
            .map(provider_secret_to_dto)
            .collect();
        Ok(ProviderSecretsListResponse { secrets })
    }

    pub(super) async fn on_delete_provider_secret(
        &self,
        req: ProviderSecretDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        match provider_secrets::delete_provider_secret(&req.id).await {
            Ok(()) => Ok(EmptyResponse {}),
            Err(provider_secrets::DeleteProviderSecretError::InvalidId(id)) => {
                Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("Invalid provider secret id: '{}'", id)))
            }
            Err(e) => Err(agent_client_protocol::Error::internal_error().data(e.to_string())),
        }
    }

    pub(super) async fn on_canonical_model_info(
        &self,
        req: CanonicalModelInfoRequest,
    ) -> Result<CanonicalModelInfoResponse, agent_client_protocol::Error> {
        use goose_providers::model::ModelConfig;

        let config_info =
            crate::providers::canonical_cost::configured_model_info(&req.provider, &req.model);
        // Config-declared prices carry the config's currency; without them the
        // response reports registry rates, which are USD.
        let currency = crate::providers::canonical_cost::display_currency(config_info.as_ref());
        let model_info =
            crate::providers::canonical::maybe_get_canonical_model(&req.provider, &req.model)
                .map(|canonical_model| {
                    // Config-declared prices outrank the registry's catalog rates;
                    // registry cache rates survive, so tooltip math matches
                    // estimate_model_cost exactly.
                    let pricing = crate::providers::canonical_cost::resolve_pricing(
                        &req.provider,
                        &req.model,
                    )
                    .unwrap_or_else(|| canonical_model.cost.clone());
                    CanonicalModelInfoDto {
                        provider: req.provider.clone(),
                        model: req.model.clone(),
                        context_limit: canonical_model.limit.context,
                        max_output_tokens: canonical_model.limit.output,
                        reasoning: canonical_model
                            .reasoning
                            .unwrap_or_else(|| ModelConfig::new(&req.model).is_reasoning_model()),
                        input_token_cost: pricing.input,
                        output_token_cost: pricing.output,
                        cache_read_token_cost: pricing.cache_read,
                        cache_write_token_cost: pricing.cache_write,
                        currency: currency.clone(),
                    }
                })
                .or_else(|| {
                    crate::providers::canonical_cost::resolve_pricing(&req.provider, &req.model)
                        .and_then(|pricing| {
                            config_info.map(|info| CanonicalModelInfoDto {
                                provider: req.provider.clone(),
                                model: req.model.clone(),
                                context_limit: info.context_limit.unwrap_or_else(|| {
                                    ModelConfig::new(&req.model).context_limit()
                                }),
                                // ModelInfo carries no max-output limit.
                                max_output_tokens: None,
                                // Configs deserialize a missing `reasoning` as false; keep
                                // name-based detection for accustomed reasoning models.
                                reasoning: info.reasoning
                                    || ModelConfig::new(&req.model).is_reasoning_model(),
                                input_token_cost: pricing.input,
                                output_token_cost: pricing.output,
                                cache_read_token_cost: pricing.cache_read,
                                cache_write_token_cost: pricing.cache_write,
                                currency: currency.clone(),
                            })
                        })
                });

        Ok(CanonicalModelInfoResponse { model_info })
    }
}

#[cfg(test)]
mod tests {
    use super::mask_secret_value;

    #[test]
    fn mask_secret_value_hides_suffix_and_never_reveals_majority() {
        for (secret, expected) in [
            ("", "***"),
            ("abcdefg", "***"),
            ("abcdefgh", "abcd..."),
            ("abcdefghijkl", "abcd..."),
        ] {
            assert_eq!(mask_secret_value(secret), expected);
        }
    }

    #[test]
    fn mask_secret_value_counts_unicode_characters() {
        assert_eq!(mask_secret_value("密碼安全令牌甲乙"), "密碼安全...");
    }
}
