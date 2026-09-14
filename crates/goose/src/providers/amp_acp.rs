use anyhow::Result;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::acp::{
    extension_configs_to_mcp_servers, AcpProvider, AcpProviderConfig, ACP_CURRENT_MODEL,
};
use crate::config::search_path::SearchPaths;
use crate::config::{Config, GooseMode};
use crate::providers::base::{
    current_working_dir, ProviderDef, ProviderDescriptor, ProviderMetadata,
};
use crate::providers::catalog::ProviderSetupMetadata;

pub(crate) const AMP_ACP_PROVIDER_NAME: &str = "amp-acp";
const AMP_ACP_DOC_URL: &str = "https://ampcode.com";
pub(crate) const AMP_ACP_BINARY: &str = "amp-acp";

pub struct AmpAcpProvider;

impl goose_providers::base::ProviderDescriptor for AmpAcpProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            AMP_ACP_PROVIDER_NAME,
            "Amp",
            "Use goose with your Amp subscription via the amp-acp adapter.",
            ACP_CURRENT_MODEL,
            vec![],
            AMP_ACP_DOC_URL,
            vec![],
        )
        .with_setup_steps(vec![
            "Install the Amp CLI: `curl -fsSL https://ampcode.com/install.sh | bash`",
            "Install the ACP adapter: `npm install -g amp-acp`",
            "Ensure your Amp CLI is authenticated (run `amp` to verify)",
        ])
        .with_setup(
            ProviderSetupMetadata::cli_agent(AMP_ACP_BINARY, &["amp-acp", "amp"])
                .with_acp()
                .with_docs_url("https://ampcode.com")
                .with_capabilities(true, true, true),
        )
    }
}

impl ProviderDef for AmpAcpProvider {
    type Provider = AcpProvider;

    fn from_env(
        extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<AcpProvider>> {
        Self::from_env_with_working_dir(extensions, current_working_dir(), tls_config)
    }

    fn from_env_with_working_dir(
        extensions: Vec<crate::config::ExtensionConfig>,
        working_dir: PathBuf,
        _tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<AcpProvider>> {
        Box::pin(async move {
            let config = Config::global();
            let resolved_command = SearchPaths::builder().with_npm().resolve(AMP_ACP_BINARY)?;
            let goose_mode = config.get_goose_mode().unwrap_or(GooseMode::Auto);

            let mode_mapping = HashMap::from([
                // "bypass" skips confirmations, closest to autonomous mode.
                (GooseMode::Auto, vec!["bypass".to_string()]),
                // "default" prompts before risky actions.
                (GooseMode::Approve, vec!["default".to_string()]),
                (GooseMode::SmartApprove, vec!["default".to_string()]),
                (GooseMode::Chat, vec!["default".to_string()]),
            ]);

            let provider_config = AcpProviderConfig {
                command: resolved_command,
                args: vec![],
                env: vec![],
                env_remove: vec![],
                work_dir: working_dir,
                mcp_servers: extension_configs_to_mcp_servers(&extensions),
                session_mode_id: mode_mapping[&goose_mode].first().cloned(),
                session_config_options: vec![],
                model_config_option_id: None,
                mode_mapping,
                notification_callback: None,
            };

            let metadata = Self::metadata();
            AcpProvider::connect(metadata.name, goose_mode, provider_config).await
        })
    }
}
