use std::path::PathBuf;
use std::sync::{Arc, RwLock};

#[cfg(feature = "aws-providers")]
use super::bedrock::BedrockProvider;
#[cfg(feature = "local-inference")]
use super::local_inference::LocalInferenceProvider;
#[cfg(feature = "aws-providers")]
use super::sagemaker_tgi::SageMakerTgiProvider;
use super::{
    amp_acp::AmpAcpProvider,
    avian::AvianProvider,
    azure::AzureProvider,
    base::{Provider, ProviderMetadata},
    chatgpt_codex::ChatGptCodexProvider,
    claude_acp::ClaudeAcpProvider,
    claude_code::ClaudeCodeProvider,
    codex::CodexProvider,
    codex_acp::CodexAcpProvider,
    copilot_acp::CopilotAcpProvider,
    cursor_agent::CursorAgentProvider,
    gcpvertexai::GcpVertexAIProvider,
    gemini_cli::GeminiCliProvider,
    gemini_oauth::GeminiOAuthProvider,
    githubcopilot::GithubCopilotProvider,
    gondola::GondolaProvider,
    huggingface::HuggingFaceProvider,
    kimicode::KimiCodeProvider,
    litellm::LiteLLMProvider,
    nanogpt::NanoGptProvider,
    pi_acp::PiAcpProvider,
    provider_registry::ProviderRegistry,
    snowflake_def::SnowflakeProviderDef,
    tetrate::TetrateProvider,
    xai::XaiProvider,
    xai_oauth::XaiOAuthProvider,
};
use crate::config::ExtensionConfig;
use crate::providers::anthropic_def::AnthropicProviderDef;
use crate::providers::azure_foundry_def::AzureFoundryProviderDef;
use crate::providers::base::ProviderType;
use crate::providers::databricks_def::{self, DatabricksProviderDef};
use crate::providers::databricks_v2_def::{self, DatabricksV2ProviderDef};
use crate::providers::google_def::GoogleProviderDef;
use crate::providers::ollama_def::OllamaProviderDef;
use crate::providers::openai_def::OpenAiProviderDef;
use crate::providers::openrouter_def::OpenRouterProviderDef;
use crate::{
    config::declarative_providers::register_declarative_providers,
    providers::provider_registry::ProviderEntry,
};
use anyhow::Result;
use tokio::sync::OnceCell;

static REGISTRY: OnceCell<RwLock<ProviderRegistry>> = OnceCell::const_new();

async fn init_registry() -> RwLock<ProviderRegistry> {
    let tls_config =
        crate::config::tls::provider_tls_config_from_config(crate::config::Config::global())
            .expect("failed to load provider TLS config");
    let mut registry = ProviderRegistry::new(tls_config).with_providers(|registry| {
        use super::inventory::registrations;

        registry.register_with_inventory::<AmpAcpProvider>(
            false,
            Some(registrations::amp_acp_inventory()),
        );
        registry.register_with_inventory::<AnthropicProviderDef>(
            true,
            Some(registrations::anthropic_inventory()),
        );
        registry.register::<AvianProvider>(false);
        registry.register::<AzureProvider>(false);
        registry.register_with_inventory::<AzureFoundryProviderDef>(
            true,
            Some(registrations::azure_foundry_inventory()),
        );
        #[cfg(feature = "aws-providers")]
        registry.register::<BedrockProvider>(false);
        #[cfg(feature = "local-inference")]
        registry.register::<LocalInferenceProvider>(false);
        registry.register_with_inventory::<ChatGptCodexProvider>(
            true,
            Some(registrations::chatgpt_codex_inventory()),
        );
        registry.register_with_inventory::<ClaudeAcpProvider>(
            false,
            Some(registrations::claude_acp_inventory()),
        );
        registry.register::<ClaudeCodeProvider>(true);
        registry.register_with_inventory::<CodexAcpProvider>(
            false,
            Some(registrations::codex_acp_inventory()),
        );
        registry.register_with_inventory::<CopilotAcpProvider>(
            false,
            Some(registrations::copilot_acp_inventory()),
        );
        registry.register::<CodexProvider>(true);
        registry.register_with_inventory::<CursorAgentProvider>(
            false,
            Some(registrations::refresh_only()),
        );
        registry.register_with_inventory::<DatabricksProviderDef>(
            true,
            Some(registrations::refresh_only()),
        );
        registry.register_with_inventory::<DatabricksV2ProviderDef>(
            false,
            Some(registrations::refresh_only()),
        );
        registry.register_with_inventory::<GcpVertexAIProvider>(
            false,
            Some(registrations::refresh_only()),
        );
        registry.register::<GeminiCliProvider>(false);
        registry.register_with_inventory::<GeminiOAuthProvider>(
            false,
            Some(registrations::gemini_oauth_inventory()),
        );
        registry.register_with_inventory::<GithubCopilotProvider>(
            false,
            Some(registrations::refresh_only()),
        );
        registry.register::<GondolaProvider>(false);
        registry.register_with_inventory::<GoogleProviderDef>(
            true,
            Some(registrations::google_inventory()),
        );
        registry.register_with_inventory::<HuggingFaceProvider>(
            true,
            Some(registrations::huggingface_inventory()),
        );
        registry.register_with_inventory::<KimiCodeProvider>(
            true,
            Some(registrations::kimi_code_inventory()),
        );
        registry.register_with_inventory::<LiteLLMProvider>(
            false,
            Some(registrations::refresh_only().with_configured(|| {
                let config = crate::config::Config::global();
                config
                    .get_param::<serde_json::Value>("LITELLM_HOST")
                    .is_ok()
                    || config
                        .get_secret::<serde_json::Value>("LITELLM_API_KEY")
                        .is_ok()
            })),
        );
        registry
            .register_with_inventory::<NanoGptProvider>(true, Some(registrations::refresh_only()));
        registry.register_with_inventory::<OllamaProviderDef>(
            true,
            Some(registrations::ollama_inventory()),
        );
        registry.register_with_inventory::<OpenAiProviderDef>(
            true,
            Some(registrations::openai_inventory()),
        );
        registry.register_with_inventory::<OpenRouterProviderDef>(
            true,
            Some(registrations::refresh_only().with_configured(|| {
                let config = crate::config::Config::global();
                config
                    .get_secret::<serde_json::Value>("OPENROUTER_API_KEY")
                    .is_ok()
            })),
        );
        registry.register_with_inventory::<PiAcpProvider>(
            false,
            Some(registrations::pi_acp_inventory()),
        );
        #[cfg(feature = "aws-providers")]
        registry.register::<SageMakerTgiProvider>(false);
        registry.register::<SnowflakeProviderDef>(false);
        registry
            .register_with_inventory::<TetrateProvider>(true, Some(registrations::refresh_only()));
        registry.register_with_inventory::<XaiProvider>(false, Some(registrations::refresh_only()));
        registry.register_with_inventory::<XaiOAuthProvider>(
            true,
            Some(registrations::xai_oauth_inventory()),
        );
    });
    // Register cleanup functions for providers with cached state
    registry.set_cleanup(
        "github_copilot",
        Arc::new(|| Box::pin(GithubCopilotProvider::cleanup())),
    );
    registry.set_cleanup(
        "databricks",
        Arc::new(|| Box::pin(databricks_def::cleanup())),
    );
    registry.set_cleanup(
        "databricks_v2",
        Arc::new(|| Box::pin(databricks_v2_def::cleanup())),
    );
    registry.set_cleanup(
        "kimi_code",
        Arc::new(|| Box::pin(KimiCodeProvider::cleanup())),
    );
    registry.set_cleanup(
        "chatgpt_codex",
        Arc::new(|| Box::pin(ChatGptCodexProvider::cleanup())),
    );
    registry.set_cleanup(
        "gemini_oauth",
        Arc::new(|| Box::pin(GeminiOAuthProvider::cleanup())),
    );
    registry.set_cleanup(
        "xai_oauth",
        Arc::new(|| Box::pin(XaiOAuthProvider::cleanup())),
    );
    registry.set_cleanup(
        "huggingface",
        Arc::new(|| Box::pin(HuggingFaceProvider::cleanup())),
    );

    if let Err(e) = load_custom_providers_into_registry(&mut registry) {
        tracing::warn!("Failed to load custom providers: {}", e);
    }
    RwLock::new(registry)
}

fn load_custom_providers_into_registry(registry: &mut ProviderRegistry) -> Result<()> {
    register_declarative_providers(registry)
}

async fn get_registry() -> &'static RwLock<ProviderRegistry> {
    REGISTRY.get_or_init(init_registry).await
}

pub async fn providers() -> Vec<(ProviderMetadata, ProviderType)> {
    get_registry()
        .await
        .read()
        .unwrap()
        .all_metadata_with_types()
}

pub async fn refresh_custom_providers() -> Result<()> {
    let registry = get_registry().await;
    registry.write().unwrap().remove_custom_providers();

    if let Err(e) = load_custom_providers_into_registry(&mut registry.write().unwrap()) {
        tracing::warn!("Failed to refresh custom providers: {}", e);
        return Err(e);
    }

    tracing::info!("Custom providers refreshed");
    Ok(())
}

pub async fn get_from_registry(name: &str) -> Result<ProviderEntry> {
    let guard = get_registry().await.read().unwrap();
    guard
        .entries
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Unknown provider: {}", name))
        .cloned()
}

pub async fn inventory_identity(name: &str) -> Result<super::inventory::InventoryIdentityInput> {
    get_from_registry(name).await?.inventory_identity()
}

pub async fn create(name: &str, extensions: Vec<ExtensionConfig>) -> Result<Arc<dyn Provider>> {
    let entry = get_from_registry(name).await?;
    entry.create(extensions).await
}

pub async fn create_with_working_dir(
    name: &str,
    extensions: Vec<ExtensionConfig>,
    working_dir: PathBuf,
) -> Result<Arc<dyn Provider>> {
    let entry = get_from_registry(name).await?;
    entry.create_with_working_dir(extensions, working_dir).await
}

pub async fn create_with_default_model(
    name: impl AsRef<str>,
    extensions: Vec<ExtensionConfig>,
) -> Result<Arc<dyn Provider>> {
    get_from_registry(name.as_ref())
        .await?
        .create_with_default_model(extensions)
        .await
}

pub async fn cleanup_provider(name: &str) -> Result<()> {
    let cleanup_fn = {
        let registry = get_registry().await.read().unwrap();
        registry
            .entries
            .get(name)
            .and_then(|entry| entry.cleanup.clone())
    };
    if let Some(cleanup) = cleanup_fn {
        return cleanup().await;
    }
    Ok(())
}

pub async fn create_with_named_model(
    provider_name: &str,
    extensions: Vec<ExtensionConfig>,
) -> Result<Arc<dyn Provider>> {
    create(provider_name, extensions).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::paths::Paths;
    use std::fs;

    #[tokio::test]
    async fn test_huggingface_provider_registry_wiring() {
        let huggingface = get_from_registry("huggingface")
            .await
            .expect("huggingface provider should be registered");
        let meta = huggingface.metadata();

        assert_eq!(huggingface.provider_type(), ProviderType::Preferred);
        assert_eq!(meta.display_name, "Hugging Face");
        assert_eq!(meta.default_model, "Qwen/Qwen3-Coder-480B-A35B-Instruct");
        assert!(meta
            .config_keys
            .iter()
            .any(|key| key.name == "HF_TOKEN" && key.secret));
    }

    #[tokio::test]
    async fn test_aimlapi_provider_registry_wiring() {
        let aimlapi = get_from_registry("aimlapi")
            .await
            .expect("aimlapi provider should be registered");
        let meta = aimlapi.metadata();

        assert_eq!(meta.name, "aimlapi");
        assert_eq!(meta.display_name, "AI/ML API");
        assert_eq!(meta.default_model, "openai/gpt-5-5");
        assert!(meta
            .config_keys
            .iter()
            .any(|key| key.name == "AIMLAPI_API_KEY" && key.secret));
    }

    #[tokio::test]
    async fn test_gondola_provider_registry_wiring() {
        let gondola = get_from_registry("gondola")
            .await
            .expect("gondola provider should be registered");
        let meta = gondola.metadata();

        assert_eq!(meta.name, "gondola");
        assert_eq!(meta.default_model, "deepseek-v4-flash");
        assert!(meta
            .config_keys
            .iter()
            .any(|key| key.name == "GONDOLA_API_KEY" && key.secret));
    }

    #[tokio::test]
    async fn test_openai_compatible_providers_config_keys() {
        let providers_list = providers().await;
        let required_api_key_cases = vec![
            ("groq", "GROQ_API_KEY"),
            ("mistral", "MISTRAL_API_KEY"),
            ("custom_deepseek", "DEEPSEEK_API_KEY"),
        ];
        for (name, expected_key) in required_api_key_cases {
            if let Some((meta, _)) = providers_list.iter().find(|(m, _)| m.name == name) {
                assert!(
                    !meta.config_keys.is_empty(),
                    "{name} provider should have config keys"
                );
                assert_eq!(
                    meta.config_keys[0].name, expected_key,
                    "First config key for {name} should be {expected_key}, got {}",
                    meta.config_keys[0].name
                );
                assert!(
                    meta.config_keys[0].required,
                    "{expected_key} should be required"
                );
                assert!(
                    meta.config_keys[0].secret,
                    "{expected_key} should be secret"
                );
            } else {
                // Provider not registered; skip test for this provider
                continue;
            }
        }

        if let Some((meta, _)) = providers_list.iter().find(|(m, _)| m.name == "openai") {
            assert!(
                !meta.config_keys.is_empty(),
                "openai provider should have config keys"
            );
            assert_eq!(
                meta.config_keys[0].name, "OPENAI_API_KEY",
                "First config key for openai should be OPENAI_API_KEY"
            );
            assert!(
                !meta.config_keys[0].required,
                "OPENAI_API_KEY should be optional for local server support"
            );
            assert!(
                meta.config_keys[0].secret,
                "OPENAI_API_KEY should be secret"
            );
        }
    }

    #[tokio::test]
    async fn test_custom_provider_context_limit_is_applied_from_file() {
        let _guard = env_lock::lock_env([("GOOSE_PATH_ROOT", None::<&str>)]);
        let temp_dir = tempfile::tempdir().expect("tempdir should be created");
        std::env::set_var("GOOSE_PATH_ROOT", temp_dir.path());

        let custom_dir = Paths::config_dir().join("custom_providers");
        fs::create_dir_all(&custom_dir).expect("custom providers dir should be created");

        let custom_inf = r#"{
  "name": "custom_inf",
  "engine": "openai",
  "display_name": "Custom Inf",
  "description": "test provider",
  "api_key_env": "",
  "base_url": "https://example.invalid/v1/chat/completions",
  "models": [
    {"name": "kimi-k2.5", "context_limit": 256000}
  ],
  "requires_auth": false
}"#;
        fs::write(custom_dir.join("custom_inf.json"), custom_inf)
            .expect("custom_inf.json should be written");

        let custom_zero = r#"{
  "name": "custom_zero",
  "engine": "openai",
  "display_name": "Custom Zero",
  "description": "test provider",
  "api_key_env": "",
  "base_url": "https://example.invalid/v1/chat/completions",
  "models": [
    {"name": "zero-model", "context_limit": 0}
  ],
  "requires_auth": false
}"#;
        fs::write(custom_dir.join("custom_zero.json"), custom_zero)
            .expect("custom_zero.json should be written");

        refresh_custom_providers()
            .await
            .expect("custom providers should refresh");

        let inf_entry = get_from_registry("custom_inf")
            .await
            .expect("custom_inf entry should exist");
        let provider = inf_entry
            .create(vec![])
            .await
            .expect("custom_inf provider should be created");
        assert_eq!(provider.get_context_limit("kimi-k2.5", None).await, 256_000);

        let zero_entry = get_from_registry("custom_zero")
            .await
            .expect("custom_zero entry should exist");
        let zero_provider = zero_entry
            .create(vec![])
            .await
            .expect("custom_zero provider should be created");
        assert_eq!(
            zero_provider.get_context_limit("zero-model", None).await,
            goose_providers::model::DEFAULT_CONTEXT_LIMIT
        );

        std::env::remove_var("GOOSE_PATH_ROOT");
    }

    #[tokio::test]
    async fn test_goose_context_limit_overrides_known_models_and_defaults() {
        let _guard = env_lock::lock_env([
            ("GOOSE_PATH_ROOT", None::<&str>),
            ("GOOSE_CONTEXT_LIMIT", Some("1000000")),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_TEMPERATURE", None::<&str>),
            ("GOOSE_TOOLSHIM", None::<&str>),
            ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ("GOOSE_THINKING_EFFORT", None::<&str>),
        ]);

        let openai = get_from_registry("openai")
            .await
            .expect("openai provider should be registered");
        let openai_provider = openai
            .create(vec![])
            .await
            .expect("openai provider should be created");
        assert_eq!(
            openai_provider
                .get_context_limit("totally-unknown-model", Some(1_000_000))
                .await,
            1_000_000
        );

        let temp_dir = tempfile::tempdir().expect("tempdir should be created");
        std::env::set_var("GOOSE_PATH_ROOT", temp_dir.path());

        let custom_dir = Paths::config_dir().join("custom_providers");
        fs::create_dir_all(&custom_dir).expect("custom providers dir should be created");

        let custom_inf = r#"{
  "name": "custom_inf",
  "engine": "openai",
  "display_name": "Custom Inf",
  "description": "test provider",
  "api_key_env": "",
  "base_url": "https://example.invalid/v1/chat/completions",
  "models": [
    {"name": "kimi-k2.5", "context_limit": 256000}
  ],
  "requires_auth": false
}"#;
        fs::write(custom_dir.join("custom_inf.json"), custom_inf)
            .expect("custom_inf.json should be written");

        refresh_custom_providers()
            .await
            .expect("custom providers should refresh");

        let inf_entry = get_from_registry("custom_inf")
            .await
            .expect("custom_inf entry should exist");
        let inf_provider = inf_entry
            .create(vec![])
            .await
            .expect("custom_inf provider should be created");
        assert_eq!(
            inf_provider
                .get_context_limit("kimi-k2.5", Some(1_000_000))
                .await,
            1_000_000
        );

        std::env::remove_var("GOOSE_PATH_ROOT");
    }

    #[tokio::test]
    async fn test_litellm_supports_inventory_refresh() {
        let entry = get_from_registry("litellm")
            .await
            .expect("litellm should be registered");
        assert!(
            entry.supports_inventory_refresh(),
            "litellm must support inventory refresh so the model picker calls fetch_supported_models"
        );
    }

    #[tokio::test]
    async fn test_api_backed_model_providers_are_registered_for_refresh() {
        for provider_name in [
            "gcp_vertex_ai",
            "github_copilot",
            "kimi_code",
            "nano-gpt",
            "tetrate",
            "xai",
            "xai_oauth",
        ] {
            let entry = get_from_registry(provider_name)
                .await
                .expect("dynamic model provider should be registered");
            assert!(
                entry.supports_inventory_refresh(),
                "{provider_name} must refresh its model inventory"
            );
        }
    }

    #[tokio::test]
    async fn test_litellm_configured_without_api_key() {
        let _guard = env_lock::lock_env([
            ("LITELLM_API_KEY", None::<&str>),
            ("LITELLM_HOST", Some("http://localhost:4000")),
        ]);

        let entry = get_from_registry("litellm")
            .await
            .expect("litellm should be registered");
        assert!(
            entry.inventory_configured(),
            "litellm should be considered configured when LITELLM_HOST is set without an API key"
        );
    }

    #[tokio::test]
    async fn test_litellm_not_configured_without_any_settings() {
        let _guard = env_lock::lock_env([
            ("LITELLM_API_KEY", None::<&str>),
            ("LITELLM_HOST", None::<&str>),
        ]);

        let entry = get_from_registry("litellm")
            .await
            .expect("litellm should be registered");
        assert!(
            !entry.inventory_configured(),
            "litellm should not be considered configured when no settings are present"
        );
    }
}
