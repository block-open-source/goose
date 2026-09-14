use anyhow::Result;
use std::sync::OnceLock;

use crate::model::ModelSettings;

pub type StringParamResolver = fn(&'static str) -> Result<Option<String>>;
pub type BoolParamResolver = fn(&'static str) -> Result<Option<bool>>;
pub type ModelSettingsResolver = fn(&str) -> Result<Option<ModelSettings>>;
pub type ModelSettingsWriter = fn(&str, &ModelSettings) -> Result<()>;

static STRING_PARAM_RESOLVER: OnceLock<StringParamResolver> = OnceLock::new();
static BOOL_PARAM_RESOLVER: OnceLock<BoolParamResolver> = OnceLock::new();
static MODEL_SETTINGS_RESOLVER: OnceLock<ModelSettingsResolver> = OnceLock::new();
static MODEL_SETTINGS_WRITER: OnceLock<ModelSettingsWriter> = OnceLock::new();

pub fn set_string_param_resolver(resolve_param: StringParamResolver) {
    let _ = STRING_PARAM_RESOLVER.set(resolve_param);
}

pub fn set_bool_param_resolver(resolve_param: BoolParamResolver) {
    let _ = BOOL_PARAM_RESOLVER.set(resolve_param);
}

pub fn set_model_settings_resolver(resolve_settings: ModelSettingsResolver) {
    let _ = MODEL_SETTINGS_RESOLVER.set(resolve_settings);
}

pub fn set_model_settings_writer(write_settings: ModelSettingsWriter) {
    let _ = MODEL_SETTINGS_WRITER.set(write_settings);
}

pub fn string_param(key: &'static str) -> Result<Option<String>> {
    match STRING_PARAM_RESOLVER.get() {
        Some(resolve_param) => resolve_param(key),
        None => Ok(None),
    }
}

pub fn bool_param(key: &'static str) -> Result<Option<bool>> {
    match BOOL_PARAM_RESOLVER.get() {
        Some(resolve_param) => resolve_param(key),
        None => Ok(None),
    }
}

pub fn model_settings(model_id: &str) -> Result<ModelSettings> {
    Ok(MODEL_SETTINGS_RESOLVER
        .get()
        .map(|resolve_settings| resolve_settings(model_id))
        .transpose()?
        .flatten()
        .unwrap_or_default())
}

pub fn write_model_settings(model_id: &str, settings: &ModelSettings) -> Result<()> {
    let writer = MODEL_SETTINGS_WRITER
        .get()
        .ok_or_else(|| anyhow::anyhow!("Model settings persistence is not configured"))?;
    writer(model_id, settings)
}
