use super::*;
use goose_providers::thinking::ThinkingEffort;

const SECRET_MASK_SHOW_LEN: usize = 8;

fn mask_secret(secret: serde_json::Value) -> String {
    let as_string = match secret {
        serde_json::Value::String(s) => s,
        _ => serde_json::to_string(&secret).unwrap_or_else(|_| secret.to_string()),
    };

    let chars: Vec<_> = as_string.chars().collect();
    let show_len = std::cmp::min(chars.len() / 2, SECRET_MASK_SHOW_LEN);
    let visible: String = chars.iter().take(show_len).collect();
    let mask = "*".repeat(chars.len() - show_len);

    format!("{}{}", visible, mask)
}

impl GooseAcpAgent {
    pub(super) async fn on_preferences_read(
        &self,
        req: PreferencesReadRequest,
    ) -> Result<PreferencesReadResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        let keys = if req.keys.is_empty() {
            PREFERENCE_DEFS.iter().map(|def| def.key).collect()
        } else {
            req.keys
        };
        let mut values = Vec::with_capacity(keys.len());

        for key in keys {
            let def = preference_def(key)?;
            let value = match config.get_param::<serde_json::Value>(def.config_key) {
                Ok(value) => value,
                Err(crate::config::ConfigError::NotFound(_)) => serde_json::Value::Null,
                Err(e) => {
                    return Err(agent_client_protocol::Error::internal_error().data(e.to_string()))
                }
            };
            values.push(PreferenceValue { key, value });
        }

        Ok(PreferencesReadResponse { values })
    }

    pub(super) async fn on_preferences_save(
        &self,
        req: PreferencesSaveRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        let mut updates = Vec::with_capacity(req.values.len());

        for preference in &req.values {
            let def = preference_def(preference.key)?;
            let value = (def.prepare)(&preference.value)?;
            updates.push((def.config_key.to_string(), value));
        }

        config.set_param_values(&updates).internal_err()?;
        Ok(EmptyResponse {})
    }

    pub(super) async fn on_config_read(
        &self,
        req: ConfigReadRequest,
    ) -> Result<ConfigReadResponse, agent_client_protocol::Error> {
        let config = self.config()?;

        if req.key == "GOOSE_PROVIDER" || req.key == "active_provider" {
            let value = config
                .get_goose_provider()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null);
            return Ok(ConfigReadResponse { value });
        }
        if req.key == "GOOSE_MODEL" {
            let value = config
                .get_goose_model()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null);
            return Ok(ConfigReadResponse { value });
        }

        let value = match config.get(&req.key, req.is_secret) {
            Ok(value) if req.is_secret => serde_json::Value::String(mask_secret(value)),
            Ok(value) => value,
            Err(crate::config::ConfigError::NotFound(_)) => serde_json::Value::Null,
            Err(e) => {
                return Err(agent_client_protocol::Error::internal_error().data(e.to_string()))
            }
        };
        Ok(ConfigReadResponse { value })
    }

    pub(super) async fn on_config_upsert(
        &self,
        req: ConfigUpsertRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        let config = self.config()?;

        if req.key == "GOOSE_PROVIDER" {
            if let Some(name) = req.value.as_str() {
                let model = crate::config::get_provider_entry(config, name)
                    .map(|e| e.model)
                    .or_else(|| config.get_goose_model().ok())
                    .unwrap_or_default();
                crate::config::set_active_provider(config, name, &model).internal_err()?;
                return Ok(EmptyResponse {});
            }
        }
        if req.key == "GOOSE_MODEL" {
            if let Some(model) = req.value.as_str() {
                if let Ok(provider) = config.get_goose_provider() {
                    crate::config::set_active_provider(config, &provider, model).internal_err()?;
                    return Ok(EmptyResponse {});
                }
            }
        }

        config
            .set(&req.key, &req.value, req.is_secret)
            .internal_err()?;
        Ok(EmptyResponse {})
    }

    pub(super) async fn on_config_remove(
        &self,
        req: ConfigRemoveRequest,
    ) -> Result<EmptyResponse, agent_client_protocol::Error> {
        let config = self.config()?;

        if req.is_secret {
            config.delete_secret(&req.key).internal_err()?;
        } else if req.key == "GOOSE_PROVIDER" || req.key == "active_provider" {
            config.delete("active_provider").internal_err()?;
            config.delete("GOOSE_PROVIDER").internal_err()?;
        } else if req.key == "GOOSE_MODEL" {
            if let Ok(provider) = config.get_goose_provider() {
                crate::config::set_active_provider(config, &provider, "").internal_err()?;
            }
            config.delete("GOOSE_MODEL").internal_err()?;
        } else {
            config.delete(&req.key).internal_err()?;
        }

        Ok(EmptyResponse {})
    }

    pub(super) async fn on_config_read_all(
        &self,
        _req: ConfigReadAllRequest,
    ) -> Result<ConfigReadAllResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        let values = config.all_values().internal_err()?;
        Ok(ConfigReadAllResponse { config: values })
    }

    pub(super) async fn on_defaults_read(
        &self,
        _req: DefaultsReadRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        Ok(DefaultsReadResponse {
            provider_id: config.get_goose_provider().ok(),
            model_id: config.get_goose_model().ok(),
        })
    }

    pub(super) async fn on_defaults_save(
        &self,
        req: DefaultsSaveRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        let provider_id = req.provider_id.trim().to_string();
        if provider_id.is_empty() {
            return Err(
                agent_client_protocol::Error::invalid_params().data("providerId cannot be empty")
            );
        }

        let model_id = req.model_id.and_then(|model| {
            let model = model.trim().to_string();
            (!model.is_empty()).then_some(model)
        });

        let entries = self
            .provider_inventory
            .entries(std::slice::from_ref(&provider_id))
            .await
            .internal_err_ctx("Failed to read provider inventory")?;
        let Some(entry) = entries
            .into_iter()
            .find(|entry| entry.provider_id == provider_id)
        else {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("Unknown provider: {provider_id}")));
        };

        if !entry.configured {
            return Err(agent_client_protocol::Error::invalid_params()
                .data(format!("Provider is not configured: {provider_id}")));
        }

        // Custom/unlisted model entry is always allowed (#7255), matching the CLI
        // and in-session model-selection paths, so a model that is simply absent
        // from the provider's (often non-exhaustive) inventory must still be
        // accepted as the default here. Local inference is the exception: its
        // models cannot be fetched on demand, so they are validated against the
        // inventory, the Hugging Face cache, or an explicit local path.
        if provider_id == "local" {
            if let Some(model_id) = model_id.as_deref() {
                let model_exists = entry.default_model == model_id
                    || entry.models.iter().any(|model| model.id == model_id)
                    || local_inference_model_exists(model_id).await?;
                if !model_exists {
                    return Err(agent_client_protocol::Error::invalid_params().data(format!(
                        "Model '{model_id}' is not available for provider '{provider_id}'"
                    )));
                }
            }
        }

        let config = self.config()?;
        let model = model_id.clone().unwrap_or_else(|| {
            crate::config::get_provider_entry(config, &provider_id)
                .map(|e| e.model)
                .unwrap_or_default()
        });
        crate::config::set_active_provider(config, &provider_id, &model)
            .internal_err_ctx("Failed to save default provider")?;

        Ok(DefaultsReadResponse {
            provider_id: Some(provider_id),
            model_id,
        })
    }

    pub(super) async fn on_defaults_clear(
        &self,
        _req: DefaultsClearRequest,
    ) -> Result<DefaultsReadResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        crate::config::clear_active_provider(config)
            .internal_err_ctx("Failed to clear default provider")?;

        Ok(DefaultsReadResponse {
            provider_id: None,
            model_id: None,
        })
    }
}

async fn local_inference_model_exists(
    model_id: &str,
) -> Result<bool, agent_client_protocol::Error> {
    #[cfg(feature = "local-inference")]
    {
        crate::providers::local_inference::management::model_exists(model_id)
            .await
            .internal_err_ctx("Failed to read local inference models")
    }

    #[cfg(not(feature = "local-inference"))]
    {
        let _ = model_id;
        Ok(false)
    }
}

struct PreferenceDef {
    key: PreferenceKey,
    config_key: &'static str,
    prepare: fn(&serde_json::Value) -> Result<serde_json::Value, agent_client_protocol::Error>,
}

const PREFERENCE_DEFS: &[PreferenceDef] = &[
    PreferenceDef {
        key: PreferenceKey::AutoCompactThreshold,
        config_key: "GOOSE_AUTO_COMPACT_THRESHOLD",
        prepare: prepare_auto_compact_threshold,
    },
    PreferenceDef {
        key: PreferenceKey::GooseThinkingEffort,
        config_key: "GOOSE_THINKING_EFFORT",
        prepare: prepare_thinking_effort,
    },
    PreferenceDef {
        key: PreferenceKey::VoiceAutoSubmitPhrases,
        config_key: "VOICE_AUTO_SUBMIT_PHRASES",
        prepare: prepare_voice_auto_submit_phrases,
    },
    PreferenceDef {
        key: PreferenceKey::VoiceDictationProvider,
        config_key: "VOICE_DICTATION_PROVIDER",
        prepare: prepare_voice_dictation_provider,
    },
    PreferenceDef {
        key: PreferenceKey::VoiceDictationPreferredMic,
        config_key: "VOICE_DICTATION_PREFERRED_MIC",
        prepare: prepare_voice_dictation_preferred_mic,
    },
];

fn preference_def(
    key: PreferenceKey,
) -> Result<&'static PreferenceDef, agent_client_protocol::Error> {
    PREFERENCE_DEFS
        .iter()
        .find(|def| def.key == key)
        .ok_or_else(|| {
            agent_client_protocol::Error::internal_error()
                .data(format!("Missing preference definition for {key:?}"))
        })
}

fn prepare_auto_compact_threshold(
    value: &serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let Some(threshold) = value.as_f64() else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("autoCompactThreshold must be a number"));
    };
    if !threshold.is_finite() || threshold <= 0.0 || threshold > 1.0 {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("autoCompactThreshold must be greater than 0 and at most 1"));
    }

    Ok(value.clone())
}

fn prepare_thinking_effort(
    value: &serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let Some(value) = value.as_str() else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("gooseThinkingEffort must be a string"));
    };
    let effort = value.parse::<ThinkingEffort>().map_err(|err| {
        agent_client_protocol::Error::invalid_params()
            .data(format!("Invalid gooseThinkingEffort: {err}"))
    })?;

    Ok(serde_json::Value::String(effort.to_string()))
}

fn prepare_voice_auto_submit_phrases(
    value: &serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    if !value.is_string() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("voiceAutoSubmitPhrases must be a string"));
    }

    Ok(value.clone())
}

fn prepare_voice_dictation_provider(
    value: &serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let Some(value) = value.as_str() else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("voiceDictationProvider must be a string"));
    };
    if !is_supported_voice_dictation_provider(value) {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("voiceDictationProvider is not supported"));
    }

    Ok(serde_json::Value::String(value.to_string()))
}

fn prepare_voice_dictation_preferred_mic(
    value: &serde_json::Value,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let Some(value) = value.as_str() else {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("voiceDictationPreferredMic must be a string"));
    };
    if value.is_empty() {
        return Err(agent_client_protocol::Error::invalid_params()
            .data("voiceDictationPreferredMic must be non-empty"));
    }

    Ok(serde_json::Value::String(value.to_string()))
}

fn is_supported_voice_dictation_provider(value: &str) -> bool {
    matches!(
        value,
        "openai" | "groq" | "elevenlabs" | "model" | "__disabled__"
    ) || {
        #[cfg(feature = "local-inference")]
        {
            value == "local"
        }
        #[cfg(not(feature = "local-inference"))]
        {
            false
        }
    }
}
