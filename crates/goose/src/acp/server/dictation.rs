use super::*;
#[cfg(feature = "local-inference")]
use crate::dictation::providers::transcribe_local;
use crate::dictation::providers::{
    all_providers, is_configured, transcribe_with_model, transcribe_with_provider,
    DictationProvider,
};
#[cfg(feature = "local-inference")]
use crate::dictation::whisper;

const OPENAI_TRANSCRIPTION_MODEL_CONFIG_KEY: &str = "OPENAI_TRANSCRIPTION_MODEL";
const GROQ_TRANSCRIPTION_MODEL_CONFIG_KEY: &str = "GROQ_TRANSCRIPTION_MODEL";
const ELEVENLABS_TRANSCRIPTION_MODEL_CONFIG_KEY: &str = "ELEVENLABS_TRANSCRIPTION_MODEL";
const OPENAI_TRANSCRIPTION_MODEL: &str = "whisper-1";
const GROQ_TRANSCRIPTION_MODEL: &str = "whisper-large-v3-turbo";
const ELEVENLABS_TRANSCRIPTION_MODEL: &str = "scribe_v1";

impl GooseAcpAgent {
    pub(super) async fn on_dictation_transcribe(
        &self,
        req: DictationTranscribeRequest,
    ) -> Result<DictationTranscribeResponse, agent_client_protocol::Error> {
        use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
        let config = crate::config::Config::global();

        #[cfg(not(feature = "local-inference"))]
        if req.provider == "local" {
            return Err(agent_client_protocol::Error::invalid_params()
                .data("Local inference is not available in this build"));
        }

        let provider: DictationProvider = serde_json::from_value(serde_json::Value::String(
            req.provider.clone(),
        ))
        .map_err(|_| {
            agent_client_protocol::Error::invalid_params()
                .data(format!("Unknown provider: {}", req.provider))
        })?;

        let audio_bytes = BASE64.decode(&req.audio).map_err(|_| {
            agent_client_protocol::Error::invalid_params().data("Invalid base64 audio data")
        })?;

        if audio_bytes.len() > 50 * 1024 * 1024 {
            return Err(
                agent_client_protocol::Error::invalid_params().data("Audio too large (max 50MB)")
            );
        }

        let extension = match req.mime_type.as_str() {
            "audio/webm" | "audio/webm;codecs=opus" => "webm",
            "audio/mp4" => "mp4",
            "audio/mpeg" | "audio/mpga" => "mp3",
            "audio/m4a" => "m4a",
            "audio/wav" | "audio/x-wav" => "wav",
            other => {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data(format!("Unsupported format: {other}")));
            }
        };

        let text = match provider {
            #[cfg(feature = "local-inference")]
            DictationProvider::Local => transcribe_local(audio_bytes).await,
            DictationProvider::ModelNative => {
                let audio_format = match extension {
                    "wav" => "wav",
                    "mp3" => "mp3",
                    "webm" => "webm",
                    "mp4" => "mp4",
                    "m4a" => "m4a",
                    _ => "wav",
                };
                transcribe_with_model(audio_bytes, audio_format).await
            }
            remote => {
                let (model_param, default_model) = dictation_transcribe_params(remote);
                let model = dictation_selected_model(config, remote)
                    .unwrap_or_else(|| default_model.to_string());
                transcribe_with_provider(
                    remote,
                    model_param.to_string(),
                    model,
                    audio_bytes,
                    extension,
                    &req.mime_type,
                )
                .await
            }
        }
        .internal_err()?;

        Ok(DictationTranscribeResponse { text })
    }

    pub(super) async fn on_dictation_config(
        &self,
        _req: DictationConfigRequest,
    ) -> Result<DictationConfigResponse, agent_client_protocol::Error> {
        let config = crate::config::Config::global();
        let mut providers = std::collections::HashMap::new();

        for def in all_providers() {
            let provider = def.provider;
            let host = if let Some(host_key) = def.host_key {
                config
                    .get(host_key, false)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            } else {
                None
            };

            let provider_key = serde_json::to_value(provider)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| format!("{:?}", provider).to_lowercase());
            providers.insert(
                provider_key,
                DictationProviderStatusEntry {
                    configured: is_configured(provider),
                    host,
                    description: def.description.to_string(),
                    uses_provider_config: def.uses_provider_config,
                    settings_path: def.settings_path.map(|s| s.to_string()),
                    config_key: if !def.uses_provider_config {
                        Some(def.config_key.to_string())
                    } else {
                        None
                    },
                    model_config_key: dictation_model_config_key(provider),
                    default_model: dictation_default_model(provider),
                    selected_model: dictation_selected_model(config, provider),
                    available_models: dictation_available_models(provider),
                },
            );
        }

        Ok(DictationConfigResponse { providers })
    }

    pub(super) async fn on_dictation_models_list(
        &self,
        _req: DictationModelsListRequest,
    ) -> Result<DictationModelsListResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            use crate::download_manager::{get_download_manager, DownloadStatus};

            let manager = get_download_manager();
            let recommended_id = whisper::recommend_model();
            let models = whisper::available_models()
                .iter()
                .map(|model| DictationLocalModelStatus {
                    id: model.id.to_string(),
                    label: model.id.to_string(),
                    description: model.description.to_string(),
                    size_mb: model.size_mb,
                    downloaded: model.is_downloaded(),
                    download_in_progress: manager
                        .get_progress(model.id)
                        .map(|progress| progress.status == DownloadStatus::Downloading)
                        .unwrap_or(false),
                    recommended: model.id == recommended_id,
                })
                .collect();

            Ok(DictationModelsListResponse { models })
        }

        #[cfg(not(feature = "local-inference"))]
        Ok(DictationModelsListResponse::default())
    }

    pub(super) async fn on_dictation_model_download(
        &self,
        _req: DictationModelDownloadRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            use crate::download_manager::get_download_manager;

            let model = whisper::get_model(&_req.model_id).ok_or_else(|| {
                agent_client_protocol::Error::invalid_params().data("Unknown model id")
            })?;
            let manager = get_download_manager();
            let model_id_for_config = model.id.to_string();

            manager
                .download_model(
                    model.id.to_string(),
                    model.url.to_string(),
                    model.local_path(),
                    Some(Box::new(move || {
                        let config = crate::config::Config::global();
                        // Only auto-select this model if the user has no model
                        // currently selected. This prevents silently switching
                        // the active model mid-session when a user downloads an
                        // additional model while one is already in use.
                        let already_selected = config
                            .get(whisper::LOCAL_WHISPER_MODEL_CONFIG_KEY, false)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .filter(|model_id| {
                                // Treat a deleted model file as no active selection
                                // so a fresh download can auto-select cleanly.
                                whisper::get_model(model_id)
                                    .is_some_and(|model| model.is_downloaded())
                            });
                        if already_selected.is_none() {
                            if let Err(e) = config.set_param(
                                whisper::LOCAL_WHISPER_MODEL_CONFIG_KEY,
                                model_id_for_config.clone(),
                            ) {
                                error!("Failed to save LOCAL_WHISPER_MODEL after download: {}", e);
                            }
                        }
                    })),
                )
                .await
                .internal_err()?;

            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        Err(agent_client_protocol::Error::invalid_params().data("Local inference not enabled"))
    }

    pub(super) async fn on_dictation_model_download_progress(
        &self,
        _req: DictationModelDownloadProgressRequest,
    ) -> Result<DictationModelDownloadProgressResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            use crate::download_manager::get_download_manager;

            let manager = get_download_manager();
            let progress =
                manager
                    .get_progress(&_req.model_id)
                    .map(|progress| DictationDownloadProgress {
                        bytes_downloaded: progress.bytes_downloaded,
                        total_bytes: progress.total_bytes,
                        progress_percent: progress.progress_percent,
                        status: serde_json::to_value(&progress.status)
                            .ok()
                            .and_then(|value| value.as_str().map(ToOwned::to_owned))
                            .unwrap_or_else(|| "unknown".to_string()),
                        error: progress.error,
                    });

            Ok(DictationModelDownloadProgressResponse { progress })
        }

        #[cfg(not(feature = "local-inference"))]
        Ok(DictationModelDownloadProgressResponse { progress: None })
    }

    pub(super) async fn on_dictation_model_cancel(
        &self,
        _req: DictationModelCancelRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            use crate::download_manager::get_download_manager;

            let manager = get_download_manager();
            manager.cancel_download(&_req.model_id).internal_err()?;

            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        Err(agent_client_protocol::Error::invalid_params().data("Local inference not enabled"))
    }

    pub(super) async fn on_dictation_model_delete(
        &self,
        _req: DictationModelDeleteRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        #[cfg(feature = "local-inference")]
        {
            let model = whisper::get_model(&_req.model_id).ok_or_else(|| {
                agent_client_protocol::Error::invalid_params().data("Unknown model id")
            })?;
            let path = model.local_path();

            if !path.exists() {
                return Err(
                    agent_client_protocol::Error::invalid_params().data("Model not downloaded")
                );
            }

            std::fs::remove_file(path).internal_err()?;

            Ok(EmptyResponse {})
        }

        #[cfg(not(feature = "local-inference"))]
        Err(agent_client_protocol::Error::invalid_params().data("Local inference not enabled"))
    }
}

fn dictation_model_config_key(provider: DictationProvider) -> Option<String> {
    match provider {
        DictationProvider::OpenAI => Some(OPENAI_TRANSCRIPTION_MODEL_CONFIG_KEY.to_string()),
        DictationProvider::Groq => Some(GROQ_TRANSCRIPTION_MODEL_CONFIG_KEY.to_string()),
        DictationProvider::ElevenLabs => {
            Some(ELEVENLABS_TRANSCRIPTION_MODEL_CONFIG_KEY.to_string())
        }
        DictationProvider::ModelNative => None,
        #[cfg(feature = "local-inference")]
        DictationProvider::Local => Some(whisper::LOCAL_WHISPER_MODEL_CONFIG_KEY.to_string()),
    }
}

/// Returns the (param_name, default_model) pair used by `transcribe_with_provider`
/// for remote dictation providers. Local inference is handled separately.
fn dictation_transcribe_params(provider: DictationProvider) -> (&'static str, &'static str) {
    match provider {
        DictationProvider::OpenAI => ("model", OPENAI_TRANSCRIPTION_MODEL),
        DictationProvider::Groq => ("model", GROQ_TRANSCRIPTION_MODEL),
        DictationProvider::ElevenLabs => ("model_id", ELEVENLABS_TRANSCRIPTION_MODEL),
        DictationProvider::ModelNative => ("", ""),
        #[cfg(feature = "local-inference")]
        DictationProvider::Local => ("", ""),
    }
}

fn dictation_default_model(provider: DictationProvider) -> Option<String> {
    match provider {
        DictationProvider::OpenAI => Some(OPENAI_TRANSCRIPTION_MODEL.to_string()),
        DictationProvider::Groq => Some(GROQ_TRANSCRIPTION_MODEL.to_string()),
        DictationProvider::ElevenLabs => Some(ELEVENLABS_TRANSCRIPTION_MODEL.to_string()),
        DictationProvider::ModelNative => crate::config::Config::global()
            .get_param::<String>("GOOSE_MODEL")
            .ok(),
        #[cfg(feature = "local-inference")]
        DictationProvider::Local => Some(whisper::recommend_model().to_string()),
    }
}

fn dictation_selected_model(config: &Config, provider: DictationProvider) -> Option<String> {
    if provider == DictationProvider::ModelNative {
        return crate::config::Config::global()
            .get_param::<String>("GOOSE_MODEL")
            .ok();
    }

    #[cfg(feature = "local-inference")]
    if provider == DictationProvider::Local {
        return config
            .get(whisper::LOCAL_WHISPER_MODEL_CONFIG_KEY, false)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|model_id| whisper::get_model(model_id).is_some())
            .or_else(|| dictation_default_model(provider));
    }

    dictation_model_config_key(provider)
        .and_then(|key| {
            config
                .get(&key, false)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
        })
        .or_else(|| dictation_default_model(provider))
}

fn dictation_available_models(provider: DictationProvider) -> Vec<DictationModelOption> {
    match provider {
        DictationProvider::OpenAI => vec![DictationModelOption {
            id: OPENAI_TRANSCRIPTION_MODEL.to_string(),
            label: "Whisper-1".to_string(),
            description: "OpenAI's hosted Whisper transcription model.".to_string(),
        }],
        DictationProvider::Groq => vec![DictationModelOption {
            id: GROQ_TRANSCRIPTION_MODEL.to_string(),
            label: "Whisper Large V3 Turbo".to_string(),
            description: "Groq's fast hosted Whisper transcription model.".to_string(),
        }],
        DictationProvider::ElevenLabs => vec![DictationModelOption {
            id: ELEVENLABS_TRANSCRIPTION_MODEL.to_string(),
            label: "Scribe v1".to_string(),
            description: "ElevenLabs' hosted speech-to-text model.".to_string(),
        }],
        DictationProvider::ModelNative => vec![],
        #[cfg(feature = "local-inference")]
        DictationProvider::Local => whisper::available_models()
            .iter()
            .map(|model| DictationModelOption {
                id: model.id.to_string(),
                label: model.id.to_string(),
                description: model.description.to_string(),
            })
            .collect(),
    }
}
