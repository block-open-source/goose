use super::api_client::{ApiClient, AuthMethod};
use super::base::{ConfigKey, ProviderDef, ProviderMetadata};
use super::openai_compatible::OpenAiCompatibleProvider;
use anyhow::Result;
use futures::future::BoxFuture;

const XAI_PROVIDER_NAME: &str = "xai";
pub const XAI_API_HOST: &str = "https://api.x.ai/v1";
pub const XAI_DEFAULT_MODEL: &str = "grok-4.5";

pub const XAI_DOC_URL: &str = "https://docs.x.ai/docs/overview";

pub struct XaiProvider;

pub fn xai_known_model_info() -> Vec<goose_providers::base::ModelInfo> {
    goose_providers::base::known_models_from_registry(XAI_PROVIDER_NAME)
}

impl goose_providers::base::ProviderDescriptor for XaiProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::with_models(
            XAI_PROVIDER_NAME,
            "xAI",
            "Grok models from xAI, including reasoning and multimodal capabilities",
            XAI_DEFAULT_MODEL,
            xai_known_model_info(),
            XAI_DOC_URL,
            vec![
                ConfigKey::new("XAI_API_KEY", true, true, None, true),
                ConfigKey::new("XAI_HOST", false, false, Some(XAI_API_HOST), false),
            ],
        )
        .with_setup(crate::providers::catalog::ProviderSetupMetadata::api_key(
            crate::providers::catalog::ProviderSetupGroup::Additional,
        ))
    }
}

impl ProviderDef for XaiProvider {
    type Provider = OpenAiCompatibleProvider;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<OpenAiCompatibleProvider>> {
        Box::pin(async move {
            let config = crate::config::Config::global();
            let api_key: String = config.get_secret("XAI_API_KEY")?;
            let host: String = config
                .get_param("XAI_HOST")
                .unwrap_or_else(|_| XAI_API_HOST.to_string());

            let api_client =
                ApiClient::new_with_tls(host, AuthMethod::BearerToken(api_key), tls_config)?
                    .with_request_builder(crate::session_context::session_id_request_builder());

            Ok(OpenAiCompatibleProvider::new(
                XAI_PROVIDER_NAME.to_string(),
                api_client,
                String::new(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goose_providers::base::ProviderDescriptor;

    #[test]
    fn current_xai_models_use_canonical_metadata() {
        let metadata = XaiProvider::metadata();

        let grok_4_5 = metadata
            .known_models
            .iter()
            .find(|model| model.name == "grok-4.5")
            .expect("grok-4.5 should be a known xAI model");
        assert_eq!(grok_4_5.context_limit, Some(500_000));
        assert!(grok_4_5.reasoning);

        let grok_4_20_non_reasoning = metadata
            .known_models
            .iter()
            .find(|model| model.name == "grok-4.20-0309-non-reasoning")
            .expect("grok-4.20 non-reasoning should be a known xAI model");
        assert!(!grok_4_20_non_reasoning.reasoning);
    }
}
