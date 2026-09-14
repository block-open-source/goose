pub use goose_providers::local_inference::*;

use crate::config::ExtensionConfig;
use crate::providers::api_client::TlsConfig;
use crate::providers::base::ProviderDef;
use anyhow::Result;
use futures::future::BoxFuture;
use std::collections::HashMap;

const LOCAL_MODEL_SETTINGS_KEY: &str = "GOOSE_LOCAL_MODEL_SETTINGS";

fn resolve_huggingface_token() -> BoxFuture<'static, Result<Option<String>>> {
    Box::pin(crate::providers::huggingface_auth::resolve_token_async())
}

fn resolve_string_param(key: &'static str) -> Result<Option<String>> {
    Ok(crate::config::Config::global()
        .get_param::<String>(key)
        .ok())
}

fn resolve_bool_param(key: &'static str) -> Result<Option<bool>> {
    Ok(crate::config::Config::global().get_param::<bool>(key).ok())
}

fn resolve_model_settings(
    model_id: &str,
) -> Result<Option<goose_providers::local_inference::model::ModelSettings>> {
    let settings = crate::config::Config::global()
        .get_param::<HashMap<String, goose_providers::local_inference::model::ModelSettings>>(
            LOCAL_MODEL_SETTINGS_KEY,
        )
        .unwrap_or_default();
    Ok(settings.get(model_id).cloned())
}

fn write_model_settings(
    model_id: &str,
    settings: &goose_providers::local_inference::model::ModelSettings,
) -> Result<()> {
    crate::config::Config::global().update_param::<HashMap<
        String,
        goose_providers::local_inference::model::ModelSettings,
    >, _, _>(LOCAL_MODEL_SETTINGS_KEY, |mut all_settings| {
        all_settings.insert(model_id.to_string(), settings.clone());
        all_settings
    })?;
    Ok(())
}

pub fn configure_local_inference() {
    huggingface_auth::set_token_resolver(resolve_huggingface_token);
    config_resolver::set_string_param_resolver(resolve_string_param);
    config_resolver::set_bool_param_resolver(resolve_bool_param);
    config_resolver::set_model_settings_resolver(resolve_model_settings);
    config_resolver::set_model_settings_writer(write_model_settings);
}

pub fn configure_huggingface_auth() {
    configure_local_inference();
}

impl ProviderDef for LocalInferenceProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<ExtensionConfig>,
        _tls_config: Option<TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>>
    where
        Self: Sized,
    {
        configure_local_inference();
        Box::pin(Self::from_env())
    }
}
