use super::hf_models::{
    self, model_id_from_repo, resolve_local_model_selection, resolve_local_model_spec,
    CachedLocalModel, HfModelInfo, HfModelVariant,
};
use super::model::{ChatTemplate, ModelSettings, SamplingConfig, ToolCallingMode};
use super::{
    available_inference_memory_bytes, builtin_chat_template_names, recommend_local_model,
    InferenceRuntime,
};
use crate::download_manager::{get_download_manager, DownloadProgress, DownloadStatus};
use anyhow::{anyhow, Result};
use goose_sdk_types::custom_requests::{
    LocalInferenceBuiltinChatTemplatesListResponse, LocalInferenceChatTemplate,
    LocalInferenceDownloadProgressDto, LocalInferenceDownloadState, LocalInferenceHfGgufFileDto,
    LocalInferenceHfModelInfoDto, LocalInferenceHfModelVariantDto,
    LocalInferenceHuggingFaceRepoVariantsResponse, LocalInferenceHuggingFaceSearchResponse,
    LocalInferenceModelDownloadRequest, LocalInferenceModelDownloadResponse,
    LocalInferenceModelDownloadStatusDto, LocalInferenceModelDto, LocalInferenceModelSettingsDto,
    LocalInferenceModelSettingsReadResponse, LocalInferenceModelSettingsUpdateResponse,
    LocalInferenceModelsListResponse, LocalInferenceSamplingConfig, LocalInferenceToolCallingMode,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

static MANAGEMENT_RUNTIME: OnceLock<Arc<InferenceRuntime>> = OnceLock::new();
static DOWNLOAD_CANCELLATIONS: OnceLock<Mutex<HashMap<String, Arc<watch::Sender<bool>>>>> =
    OnceLock::new();

#[derive(Clone)]
struct LocalModelSelection {
    repo_id: String,
    backend_id: String,
    variant_id: Option<String>,
}

pub async fn list_models() -> Result<LocalInferenceModelsListResponse> {
    let runtime = management_runtime()?;
    let mut active_downloads: HashMap<_, _> = get_download_manager()
        .list_progress()
        .into_iter()
        .filter(|progress| progress.status == DownloadStatus::Downloading)
        .filter_map(|progress| {
            let model_id = progress.model_id.strip_suffix("-model")?.to_string();
            Some((model_id, progress))
        })
        .collect();
    let cached_models = hf_models::cached_local_models().await?;
    let recommended_id = recommend_local_model(&runtime, &cached_models);

    let loaded_model_ids = crate::loaded_model_ids()
        .await
        .map_err(|error| anyhow!(error.to_string()))?;
    let mut models: Vec<LocalInferenceModelDto> = cached_models
        .iter()
        .map(|model| {
            let mut dto = local_model_to_dto(model, recommended_id.as_deref(), &loaded_model_ids);
            if let Some(progress) = active_downloads.remove(&model.id) {
                dto.status = active_download_status(&progress);
            }
            dto
        })
        .collect();
    models.extend(
        active_downloads
            .into_iter()
            .map(|(model_id, progress)| active_download_to_dto(model_id, &progress)),
    );
    models.sort_by(|a, b| a.id.cmp(&b.id));

    Ok(LocalInferenceModelsListResponse { models })
}

pub async fn search_huggingface_models(
    query: String,
    limit: Option<usize>,
) -> Result<LocalInferenceHuggingFaceSearchResponse> {
    let limit = limit.unwrap_or(20).min(50);
    let models = hf_models::search_local_models(&query, limit)
        .await?
        .into_iter()
        .map(hf_model_info_to_dto)
        .collect();
    Ok(LocalInferenceHuggingFaceSearchResponse { models })
}

pub async fn huggingface_repo_variants(
    repo_id: String,
) -> Result<LocalInferenceHuggingFaceRepoVariantsResponse> {
    let variants = hf_models::get_repo_local_variants(&repo_id).await?;

    let runtime = management_runtime()?;
    let available_memory = available_inference_memory_bytes(&runtime);
    let gguf_variants: Vec<_> = variants
        .iter()
        .filter(|variant| variant.backend_id == "llamacpp")
        .map(|variant| hf_models::HfQuantVariant {
            quantization: variant.variant_id.clone(),
            size_bytes: variant.size_bytes,
            filename: variant.filename.clone().unwrap_or_default(),
            download_url: variant.download_url.clone().unwrap_or_default(),
            description: "",
            quality_rank: variant.quality_rank,
            sharded: variant.sharded,
        })
        .collect();
    let recommended_index = hf_models::recommend_variant(&gguf_variants, available_memory);

    let cached_models = hf_models::cached_local_models().await?;
    let downloaded: Vec<_> = cached_models
        .iter()
        .filter(|model| model.repo_id == repo_id)
        .collect();
    let downloaded_quants = downloaded
        .iter()
        .filter(|model| model.backend_id == "llamacpp")
        .map(|model| model.quantization.clone())
        .collect();
    let downloaded_variants = downloaded.iter().map(|model| model.id.clone()).collect();

    Ok(LocalInferenceHuggingFaceRepoVariantsResponse {
        variants: variants.into_iter().map(hf_model_variant_to_dto).collect(),
        recommended_index,
        available_memory_bytes: available_memory,
        downloaded_quants,
        downloaded_variants,
    })
}

pub async fn download_model(
    req: LocalInferenceModelDownloadRequest,
) -> Result<LocalInferenceModelDownloadResponse> {
    let selection = explicit_model_selection(&req)?;
    let model_id = local_model_id_from_request(&req, selection.as_ref()).await?;
    let download_id = format!("{}-model", model_id);
    let download_reserved = get_download_manager().reserve_download(DownloadProgress {
        model_id: download_id.clone(),
        status: DownloadStatus::Downloading,
        bytes_downloaded: 0,
        total_bytes: 0,
        progress_percent: 0.0,
        speed_bps: None,
        eta_seconds: None,
        error: None,
        task_exited: false,
    })?;
    if !download_reserved {
        return Ok(LocalInferenceModelDownloadResponse { model_id });
    }

    let spec = req.spec.clone();
    let selection_for_task = selection.clone();
    let model_id_for_task = model_id.clone();
    let cancellation = register_download_cancellation(&download_id);
    let mut cancellation_rx = cancellation.subscribe();
    tokio::spawn(async move {
        let resolve = async {
            if let Some(selection) = selection_for_task {
                resolve_local_model_selection(
                    &selection.repo_id,
                    &selection.backend_id,
                    selection.variant_id.as_deref(),
                )
                .await
            } else {
                resolve_local_model_spec(&spec).await
            }
        };
        tokio::pin!(resolve);

        tokio::select! {
            biased;
            _ = cancellation_rx.changed() => {}
            resolved = &mut resolve => {
                if let Err(error) = resolved {
                    mark_download_failed(&model_id_for_task, error);
                }
            }
        }

        unregister_download_cancellation(&download_id, &cancellation);
        mark_download_task_exited(&model_id_for_task);
    });

    Ok(LocalInferenceModelDownloadResponse { model_id })
}

pub fn download_progress(model_id: &str) -> Result<Option<LocalInferenceDownloadProgressDto>> {
    Ok(get_download_manager()
        .get_progress(&format!("{}-model", model_id))
        .map(download_progress_to_dto))
}

pub fn cancel_download(model_id: &str) -> Result<()> {
    let download_id = format!("{}-model", model_id);
    get_download_manager().cancel_download(&download_id)?;
    if let Some(cancellation) = download_cancellations()
        .lock()
        .expect("download cancellation lock poisoned")
        .get(&download_id)
        .cloned()
    {
        cancellation.send_replace(true);
    }
    Ok(())
}

pub async fn delete_model(model_id: &str) -> Result<()> {
    if crate::explicit_model_path(model_id)?.is_some() {
        anyhow::bail!(
            "Model '{}' was loaded from a user-owned path and cannot be deleted by Goose",
            model_id
        );
    }
    hf_models::delete_cached_local_model(model_id).await
}

pub async fn model_exists(model_id: &str) -> Result<bool> {
    if crate::explicit_model_path(model_id)?.is_some() {
        return Ok(true);
    }
    Ok(hf_models::cached_local_model(model_id).await?.is_some())
}

pub async fn evict_model(model_id: &str) -> Result<()> {
    crate::evict_model(model_id)
        .await
        .map(|_| ())
        .map_err(|error| anyhow!(error.to_string()))
}

pub fn get_model_settings(model_id: &str) -> Result<LocalInferenceModelSettingsReadResponse> {
    let settings = crate::config_resolver::model_settings(model_id)?;
    Ok(LocalInferenceModelSettingsReadResponse {
        settings: model_settings_to_dto(&settings),
    })
}

pub fn update_model_settings(
    model_id: &str,
    settings: LocalInferenceModelSettingsDto,
) -> Result<LocalInferenceModelSettingsUpdateResponse> {
    let settings = model_settings_from_dto(settings);
    crate::config_resolver::write_model_settings(model_id, &settings)?;
    Ok(LocalInferenceModelSettingsUpdateResponse {
        settings: model_settings_to_dto(&settings),
    })
}

pub fn list_builtin_chat_templates() -> LocalInferenceBuiltinChatTemplatesListResponse {
    LocalInferenceBuiltinChatTemplatesListResponse {
        templates: builtin_chat_template_names(),
    }
}

fn management_runtime() -> Result<Arc<InferenceRuntime>> {
    if let Some(runtime) = MANAGEMENT_RUNTIME.get() {
        return Ok(runtime.clone());
    }

    let runtime = InferenceRuntime::get_or_init()?;
    match MANAGEMENT_RUNTIME.set(runtime.clone()) {
        Ok(()) => Ok(runtime),
        Err(_) => Ok(MANAGEMENT_RUNTIME
            .get()
            .expect("local inference management runtime initialized by another thread")
            .clone()),
    }
}

fn local_model_to_dto(
    model: &CachedLocalModel,
    recommended_id: Option<&str>,
    loaded_model_ids: &HashSet<String>,
) -> LocalInferenceModelDto {
    let mut settings = crate::config_resolver::model_settings(&model.id).unwrap_or_default();
    settings.backend_id = Some(model.backend_id.clone());
    settings.vision_capable = model.mmproj_path.is_some();
    settings.mmproj_size_bytes = model.mmproj_size_bytes;
    LocalInferenceModelDto {
        id: model.id.clone(),
        repo_id: model.repo_id.clone(),
        filename: model.filename.clone(),
        quantization: model.quantization.clone(),
        size_bytes: model.size_bytes,
        status: LocalInferenceModelDownloadStatusDto {
            state: LocalInferenceDownloadState::Downloaded,
            ..Default::default()
        },
        recommended: recommended_id == Some(model.id.as_str()),
        is_loaded: loaded_model_ids.contains(&model.id),
        settings: model_settings_to_dto(&settings),
        vision_capable: settings.vision_capable,
        mmproj_status: settings
            .vision_capable
            .then_some(LocalInferenceModelDownloadStatusDto {
                state: LocalInferenceDownloadState::Downloaded,
                ..Default::default()
            }),
    }
}

fn active_download_to_dto(model_id: String, progress: &DownloadProgress) -> LocalInferenceModelDto {
    let (repo_id, quantization, backend_id) = match hf_models::parse_model_spec(&model_id) {
        Ok((repo_id, quantization)) => (repo_id, quantization, "llamacpp".to_string()),
        Err(_) => (model_id.clone(), "default".to_string(), "mlx".to_string()),
    };
    let mut settings = crate::config_resolver::model_settings(&model_id).unwrap_or_default();
    settings.backend_id = Some(backend_id);
    LocalInferenceModelDto {
        id: model_id,
        repo_id,
        filename: String::new(),
        quantization,
        size_bytes: progress.total_bytes,
        status: active_download_status(progress),
        recommended: false,
        is_loaded: false,
        settings: model_settings_to_dto(&settings),
        vision_capable: false,
        mmproj_status: None,
    }
}

fn active_download_status(progress: &DownloadProgress) -> LocalInferenceModelDownloadStatusDto {
    LocalInferenceModelDownloadStatusDto {
        state: LocalInferenceDownloadState::Downloading,
        progress_percent: Some(progress.progress_percent),
        bytes_downloaded: Some(progress.bytes_downloaded),
        total_bytes: Some(progress.total_bytes),
        speed_bps: progress.speed_bps,
    }
}

fn download_progress_to_dto(progress: DownloadProgress) -> LocalInferenceDownloadProgressDto {
    LocalInferenceDownloadProgressDto {
        model_id: progress.model_id,
        status: serde_json::to_value(progress.status)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "unknown".to_string()),
        bytes_downloaded: progress.bytes_downloaded,
        total_bytes: progress.total_bytes,
        progress_percent: progress.progress_percent,
        speed_bps: progress.speed_bps,
        eta_seconds: progress.eta_seconds,
        error: progress.error,
        task_exited: progress.task_exited,
    }
}

fn hf_model_info_to_dto(model: HfModelInfo) -> LocalInferenceHfModelInfoDto {
    LocalInferenceHfModelInfoDto {
        repo_id: model.repo_id,
        author: model.author,
        model_name: model.model_name,
        downloads: model.downloads,
        gguf_files: model
            .gguf_files
            .into_iter()
            .map(|file| LocalInferenceHfGgufFileDto {
                filename: file.filename,
                size_bytes: file.size_bytes,
                quantization: file.quantization,
                download_url: file.download_url,
            })
            .collect(),
        variants: model
            .variants
            .into_iter()
            .map(hf_model_variant_to_dto)
            .collect(),
    }
}

fn hf_model_variant_to_dto(variant: HfModelVariant) -> LocalInferenceHfModelVariantDto {
    LocalInferenceHfModelVariantDto {
        variant_id: variant.variant_id,
        label: variant.label,
        backend_id: variant.backend_id,
        format: variant.format,
        model_id: variant.model_id,
        download_id: variant.download_id,
        size_bytes: variant.size_bytes,
        filename: variant.filename,
        download_url: variant.download_url,
        description: variant.description,
        quality_rank: variant.quality_rank,
        sharded: variant.sharded,
        supported: variant.supported,
        unsupported_reason: variant.unsupported_reason,
    }
}

pub fn model_settings_to_dto(settings: &ModelSettings) -> LocalInferenceModelSettingsDto {
    LocalInferenceModelSettingsDto {
        backend_id: settings.backend_id.clone(),
        context_size: settings.context_size,
        max_output_tokens: settings.max_output_tokens,
        draft_model: settings.draft_model.clone(),
        sampling: sampling_to_dto(&settings.sampling),
        repeat_penalty: settings.repeat_penalty,
        repeat_last_n: settings.repeat_last_n,
        frequency_penalty: settings.frequency_penalty,
        presence_penalty: settings.presence_penalty,
        n_batch: settings.n_batch,
        n_gpu_layers: settings.n_gpu_layers,
        use_mlock: settings.use_mlock,
        flash_attention: settings.flash_attention,
        n_threads: settings.n_threads,
        tool_calling: tool_calling_to_dto(settings.tool_calling),
        chat_template: chat_template_to_dto(&settings.chat_template),
        enable_thinking: settings.enable_thinking,
        vision_capable: settings.vision_capable,
        image_token_estimate: settings.image_token_estimate,
        mmproj_size_bytes: settings.mmproj_size_bytes,
    }
}

pub fn model_settings_from_dto(settings: LocalInferenceModelSettingsDto) -> ModelSettings {
    ModelSettings {
        backend_id: settings.backend_id,
        context_size: settings.context_size,
        max_output_tokens: settings.max_output_tokens,
        draft_model: settings.draft_model,
        sampling: sampling_from_dto(settings.sampling),
        repeat_penalty: settings.repeat_penalty,
        repeat_last_n: settings.repeat_last_n,
        frequency_penalty: settings.frequency_penalty,
        presence_penalty: settings.presence_penalty,
        n_batch: settings.n_batch,
        n_gpu_layers: settings.n_gpu_layers,
        use_mlock: settings.use_mlock,
        flash_attention: settings.flash_attention,
        n_threads: settings.n_threads,
        tool_calling: tool_calling_from_dto(settings.tool_calling),
        chat_template: chat_template_from_dto(settings.chat_template),
        enable_thinking: settings.enable_thinking,
        vision_capable: settings.vision_capable,
        image_token_estimate: settings.image_token_estimate,
        mmproj_size_bytes: settings.mmproj_size_bytes,
    }
}

fn sampling_to_dto(sampling: &SamplingConfig) -> LocalInferenceSamplingConfig {
    match sampling {
        SamplingConfig::Greedy => LocalInferenceSamplingConfig::Greedy,
        SamplingConfig::Temperature {
            temperature,
            top_k,
            top_p,
            min_p,
            seed,
        } => LocalInferenceSamplingConfig::Temperature {
            temperature: *temperature,
            top_k: *top_k,
            top_p: *top_p,
            min_p: *min_p,
            seed: *seed,
        },
        SamplingConfig::MirostatV2 { tau, eta, seed } => LocalInferenceSamplingConfig::MirostatV2 {
            tau: *tau,
            eta: *eta,
            seed: *seed,
        },
    }
}

fn sampling_from_dto(sampling: LocalInferenceSamplingConfig) -> SamplingConfig {
    match sampling {
        LocalInferenceSamplingConfig::Greedy => SamplingConfig::Greedy,
        LocalInferenceSamplingConfig::Temperature {
            temperature,
            top_k,
            top_p,
            min_p,
            seed,
        } => SamplingConfig::Temperature {
            temperature,
            top_k,
            top_p,
            min_p,
            seed,
        },
        LocalInferenceSamplingConfig::MirostatV2 { tau, eta, seed } => {
            SamplingConfig::MirostatV2 { tau, eta, seed }
        }
    }
}

fn tool_calling_to_dto(mode: ToolCallingMode) -> LocalInferenceToolCallingMode {
    match mode {
        ToolCallingMode::Auto => LocalInferenceToolCallingMode::Auto,
        ToolCallingMode::ForceNative => LocalInferenceToolCallingMode::ForceNative,
        ToolCallingMode::ForceEmulated => LocalInferenceToolCallingMode::ForceEmulated,
    }
}

fn tool_calling_from_dto(mode: LocalInferenceToolCallingMode) -> ToolCallingMode {
    match mode {
        LocalInferenceToolCallingMode::Auto => ToolCallingMode::Auto,
        LocalInferenceToolCallingMode::ForceNative => ToolCallingMode::ForceNative,
        LocalInferenceToolCallingMode::ForceEmulated => ToolCallingMode::ForceEmulated,
    }
}

fn chat_template_to_dto(template: &ChatTemplate) -> LocalInferenceChatTemplate {
    match template {
        ChatTemplate::Embedded => LocalInferenceChatTemplate::Embedded,
        ChatTemplate::Builtin { name } => {
            LocalInferenceChatTemplate::Builtin { name: name.clone() }
        }
        ChatTemplate::CustomInline { template } => LocalInferenceChatTemplate::CustomInline {
            template: template.clone(),
        },
    }
}

fn chat_template_from_dto(template: LocalInferenceChatTemplate) -> ChatTemplate {
    match template {
        LocalInferenceChatTemplate::Embedded => ChatTemplate::Embedded,
        LocalInferenceChatTemplate::Builtin { name } => ChatTemplate::Builtin { name },
        LocalInferenceChatTemplate::CustomInline { template } => {
            ChatTemplate::CustomInline { template }
        }
    }
}

fn explicit_model_selection(
    req: &LocalInferenceModelDownloadRequest,
) -> Result<Option<LocalModelSelection>> {
    if let Some(backend_id) = req.backend_id.as_deref() {
        let (repo_id, parsed_variant_id) = hf_models::parse_model_spec(&req.spec)
            .map(|(repo_id, quantization)| (repo_id, Some(quantization)))
            .unwrap_or_else(|_| (req.spec.clone(), None));
        let variant_id = req.variant_id.clone().or(parsed_variant_id);
        match backend_id {
            "mlx" => Ok(Some(LocalModelSelection {
                repo_id,
                backend_id: backend_id.to_string(),
                variant_id,
            })),
            "llamacpp" => Ok(Some(LocalModelSelection {
                repo_id,
                backend_id: backend_id.to_string(),
                variant_id: variant_id
                    .map(|variant_id| hf_models::canonicalize_quantization(&variant_id)),
            })),
            _ => anyhow::bail!("Unknown local inference backend '{}'", backend_id),
        }
    } else {
        Ok(None)
    }
}

async fn local_model_id_from_request(
    req: &LocalInferenceModelDownloadRequest,
    selection: Option<&LocalModelSelection>,
) -> Result<String> {
    if let Some(selection) = selection {
        return match selection.backend_id.as_str() {
            "mlx" => Ok(selection.repo_id.clone()),
            "llamacpp" => {
                let quantization = selection.variant_id.as_deref().ok_or_else(|| {
                    anyhow!(
                        "llama.cpp model '{}' is missing a quantization",
                        selection.repo_id
                    )
                })?;
                Ok(model_id_from_repo(
                    &selection.repo_id,
                    &hf_models::canonicalize_quantization(quantization),
                ))
            }
            _ => anyhow::bail!("Unknown local inference backend '{}'", selection.backend_id),
        };
    }

    if let Ok((repo_id, quantization)) = hf_models::parse_model_spec(&req.spec) {
        return Ok(model_id_from_repo(
            &repo_id,
            &hf_models::canonicalize_quantization(&quantization),
        ));
    }

    let variants = hf_models::get_repo_local_variants(&req.spec).await?;
    let has_llamacpp = variants
        .iter()
        .any(|variant| variant.backend_id == "llamacpp");
    let mlx_variants: Vec<_> = variants
        .iter()
        .filter(|variant| variant.backend_id == "mlx")
        .collect();
    if mlx_variants.len() == 1 && !has_llamacpp {
        Ok(req.spec.clone())
    } else {
        anyhow::bail!(
            "Model spec '{}' is ambiguous; choose one of: {}",
            req.spec,
            variants
                .iter()
                .map(|variant| variant.download_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn mark_download_failed(model_id: &str, error: impl std::fmt::Display) {
    let manager = get_download_manager();
    let download_id = format!("{}-model", model_id);
    if manager.get_progress(&download_id).is_none() {
        manager.set_progress(DownloadProgress {
            model_id: download_id.clone(),
            status: DownloadStatus::Failed,
            bytes_downloaded: 0,
            total_bytes: 0,
            progress_percent: 0.0,
            speed_bps: None,
            eta_seconds: None,
            error: Some(error.to_string()),
            task_exited: true,
        });
        return;
    }

    manager.update_progress(&download_id, |progress| {
        if progress.status != DownloadStatus::Cancelled {
            progress.status = DownloadStatus::Failed;
            progress.error = Some(error.to_string());
        }
        progress.task_exited = true;
    });
}

fn download_cancellations() -> &'static Mutex<HashMap<String, Arc<watch::Sender<bool>>>> {
    DOWNLOAD_CANCELLATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_download_cancellation(download_id: &str) -> Arc<watch::Sender<bool>> {
    let (sender, _) = watch::channel(false);
    let sender = Arc::new(sender);
    download_cancellations()
        .lock()
        .expect("download cancellation lock poisoned")
        .insert(download_id.to_string(), sender.clone());
    sender
}

fn unregister_download_cancellation(download_id: &str, cancellation: &Arc<watch::Sender<bool>>) {
    let mut cancellations = download_cancellations()
        .lock()
        .expect("download cancellation lock poisoned");
    if cancellations
        .get(download_id)
        .is_some_and(|current| Arc::ptr_eq(current, cancellation))
    {
        cancellations.remove(download_id);
    }
}

fn mark_download_task_exited(model_id: &str) {
    get_download_manager().update_progress(&format!("{}-model", model_id), |progress| {
        progress.task_exited = true;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_preserves_defaults() {
        let settings = ModelSettings::default();
        let dto = model_settings_to_dto(&settings);
        let round_trip = model_settings_from_dto(dto);
        assert_eq!(round_trip.repeat_penalty, settings.repeat_penalty);
        assert_eq!(round_trip.repeat_last_n, settings.repeat_last_n);
        assert_eq!(round_trip.enable_thinking, settings.enable_thinking);
        assert_eq!(
            round_trip.image_token_estimate,
            settings.image_token_estimate
        );
    }

    #[tokio::test]
    async fn explicit_llamacpp_selection_derives_quantized_model_id() {
        let req = LocalInferenceModelDownloadRequest {
            spec: "test/repo".to_string(),
            backend_id: Some("llamacpp".to_string()),
            variant_id: Some("q4_k_m".to_string()),
        };
        let selection = explicit_model_selection(&req).unwrap();
        let model_id = local_model_id_from_request(&req, selection.as_ref())
            .await
            .unwrap();
        assert_eq!(model_id, "test/repo:Q4_K_M");
    }
}
