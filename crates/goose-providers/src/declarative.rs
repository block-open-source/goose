#[macro_use]
mod macros;

use std::{collections::HashMap, path::Path, str::FromStr};

use anyhow::Result;
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Deserializer, Serialize};

pub static FIXED_PROVIDERS: Dir = include_dir!("$CARGO_MANIFEST_DIR/src/declarative/definitions");

pub(crate) mod declarative_providers {
    use super::*;

    expose_declarative_providers!(
        aimlapi,
        alibaba,
        atomic_chat,
        celeris,
        cerebras,
        deepseek,
        empiriolabs,
        fireworks,
        friendli,
        futurmix,
        groq,
        iflytek,
        iflytek_astron,
        inception,
        llama_swap,
        lmstudio,
        lynkr,
        meta,
        minimax,
        mistral,
        moonshot,
        nearai,
        novita,
        nvidia,
        ollama_cloud,
        omlx,
        opencode_go,
        opencode_zen,
        opper,
        orcarouter,
        ovhcloud,
        perplexity,
        pleumrouter,
        routstr,
        sakana,
        saladcloud,
        saygm,
        scaleway,
        tanzu,
        tensorix,
        together,
        trustedrouter,
        venice,
        vercel_ai_gateway,
        zai,
        zhipu,
    );
}

use crate::{
    anthropic,
    api_client::TlsConfig,
    base::{ModelInfo, Provider},
    ollama, openai,
};

pub fn fixed_provider_configs() -> anyhow::Result<Vec<DeclarativeProviderConfig>> {
    declarative_providers::fixed_provider_configs()
}

pub fn fixed_provider_config_entries() -> Vec<(&'static str, &'static str)> {
    declarative_providers::fixed_provider_config_entries()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarConfig {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    /// Defaults to the value of `required` if not specified.
    /// UIs may use this to feature this config value more prominently.
    pub primary: Option<bool>,
    pub description: Option<String>,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderEngine {
    #[serde(alias = "openai_compatible")]
    OpenAI,
    #[serde(alias = "ollama_compatible")]
    Ollama,
    #[serde(alias = "anthropic_compatible")]
    Anthropic,
}

impl FromStr for ProviderEngine {
    type Err = anyhow::Error;

    fn from_str(engine: &str) -> Result<Self> {
        match engine.trim().to_lowercase().as_str() {
            "openai" | "openai_compatible" => Ok(Self::OpenAI),
            "anthropic" | "anthropic_compatible" => Ok(Self::Anthropic),
            "ollama" | "ollama_compatible" => Ok(Self::Ollama),
            _ => Err(anyhow::anyhow!("Invalid provider type: {}", engine)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Executed directly, never through a shell, so `args` is never
    /// shell-interpolated. For shell features, invoke an interpreter
    /// explicitly, e.g. `command: "/bin/bash"`, `args: ["-c", "..."]`.
    /// Bare names (no path separator) are resolved via `PATH`; paths are
    /// resolved against `cwd`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// How long a fetched credential is cached before the command is re-run,
    /// in seconds. `0` disables proactive refresh entirely — the command
    /// only reruns reactively, after an auth failure (matches Codex's
    /// `refresh_interval_ms: 0` convention).
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,
    /// Timeout for the command, in seconds. Defaults to 10s if unset.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Working directory for the command, and the base a relative `command`
    /// path is resolved against. Defaults to goose's current directory.
    #[serde(default)]
    pub cwd: Option<String>,
}

fn default_refresh_interval() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclarativeProviderConfig {
    pub name: String,
    pub engine: ProviderEngine,
    pub display_name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub api_key_env: String,
    pub base_url: String,
    pub models: Vec<ModelInfo>,
    pub headers: Option<HashMap<String, String>>,
    /// Overrides the default `agent-session-id` header name for session ID propagation.
    #[serde(default)]
    pub session_id_header_override: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub supports_streaming: Option<bool>,
    #[serde(default = "default_requires_auth")]
    pub requires_auth: bool,
    #[serde(default)]
    pub catalog_provider_id: Option<String>,
    #[serde(default)]
    pub base_path: Option<String>,
    #[serde(default)]
    pub env_vars: Option<Vec<EnvVarConfig>>,
    /// Alternative to `api_key_env`: run a command to fetch/refresh the credential
    /// instead of reading a static secret. Mutually exclusive with `api_key_env`.
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    /// Controls whether `fetch_supported_models` calls the provider's `/v1/models`
    /// endpoint or returns the static `models` list directly.
    ///
    /// - `Some(false)` + non-empty `models`: return the static list; no API call.
    ///   Construction fails if `models` is empty.
    /// - `Some(true)` or `None`: try the API; fall back to `models` on 404.
    #[serde(default)]
    pub dynamic_models: Option<bool>,
    #[serde(default)]
    pub skip_canonical_filtering: bool,
    #[serde(default, deserialize_with = "deserialize_non_empty_string")]
    pub model_doc_link: Option<String>,
    #[serde(default)]
    pub setup_steps: Vec<String>,
    #[serde(default)]
    pub toolshim: bool,
    #[serde(default)]
    pub preserves_thinking: bool,
    /// Enables Z.AI's `clear_thinking` field, which Anthropic does not support.
    #[serde(default)]
    pub emit_clear_thinking: bool,
    #[serde(default)]
    pub setup: Option<goose_provider_types::canonical::catalog::ProviderSetupMetadata>,
}

fn default_requires_auth() -> bool {
    true
}

pub fn should_preserve_thinking_by_default(engine: &ProviderEngine) -> bool {
    matches!(engine, ProviderEngine::OpenAI)
}

/// Deserialize an optional string, treating empty/whitespace-only values as None.
fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.trim().is_empty()))
}

impl DeclarativeProviderConfig {
    pub fn id(&self) -> &str {
        &self.name
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// Errors if both `api_key_env` and `auth.command` are set; they're
    /// alternative ways to authenticate and mutually exclusive.
    pub fn validate_auth(&self) -> anyhow::Result<()> {
        if self.auth.is_some() && !self.api_key_env.is_empty() {
            anyhow::bail!(
                "Provider '{}' sets both `api_key_env` and `auth.command`; these are mutually exclusive.",
                self.name
            );
        }
        Ok(())
    }
}

pub trait KeyResolver {
    type Error: std::error::Error + Send + Sync + 'static;

    fn resolve_key(&self, key: &str) -> std::result::Result<String, Self::Error>;
}

pub struct EnvKeyResolver;

impl EnvKeyResolver {
    pub fn new() -> Self {
        EnvKeyResolver {}
    }
}

impl Default for EnvKeyResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyResolver for EnvKeyResolver {
    type Error = std::env::VarError;

    fn resolve_key(&self, key: &str) -> std::result::Result<String, Self::Error> {
        std::env::var(key)
    }
}

fn expand_env_vars(template: &str, env_vars: &[EnvVarConfig]) -> Result<String> {
    let mut result = template.to_string();

    for var in env_vars {
        let placeholder = format!("${{{}}}", var.name);
        if !result.contains(&placeholder) {
            continue;
        }

        let value = match std::env::var(&var.name) {
            Ok(value) => value,
            Err(_) => match &var.default {
                Some(default) => default.clone(),
                None if var.required => {
                    anyhow::bail!("Required environment variable {} is not set", var.name)
                }
                None => continue,
            },
        };

        result = result.replace(&placeholder, &value);
    }

    Ok(result)
}

fn resolve_config(config: &mut DeclarativeProviderConfig) -> Result<()> {
    if let Some(env_vars) = &config.env_vars {
        config.base_url = expand_env_vars(&config.base_url, env_vars)?;

        for var in env_vars {
            if var.name.ends_with("_STREAMING") {
                let value = std::env::var(&var.name)
                    .ok()
                    .or_else(|| var.default.clone())
                    .map(|value| value.eq_ignore_ascii_case("true"));
                if let Some(value) = value {
                    config.supports_streaming = Some(value);
                }
            }
        }
    }

    Ok(())
}

pub fn deserialize_provider_config(json: &str) -> Result<DeclarativeProviderConfig> {
    let raw: serde_json::Value = serde_json::from_str(json)?;
    let preserves_thinking_was_set = raw.get("preserves_thinking").is_some();
    let mut config: DeclarativeProviderConfig = serde_json::from_value(raw)?;

    if !preserves_thinking_was_set {
        config.preserves_thinking = should_preserve_thinking_by_default(&config.engine);
    }

    Ok(config)
}

fn config_from_json(json: &str) -> Result<DeclarativeProviderConfig> {
    let mut config = deserialize_provider_config(json)?;
    resolve_config(&mut config)?;
    Ok(config)
}

pub fn load_custom_providers(dir: &Path) -> Result<Vec<DeclarativeProviderConfig>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    std::fs::read_dir(dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "json").then_some(path)
        })
        .map(|path| {
            let content = std::fs::read_to_string(&path)?;
            deserialize_provider_config(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))
        })
        .collect()
}

pub fn from_json(
    json: &str,
    tls_config: Option<TlsConfig>,
    key_resolver: impl KeyResolver,
) -> Result<Box<dyn Provider>> {
    let config = config_from_json(json)?;

    match config.engine {
        ProviderEngine::OpenAI => openai::from_declarative_config(config, tls_config, key_resolver)
            .map(|provider| Box::new(provider.build()) as Box<dyn Provider>),
        ProviderEngine::Ollama => ollama::from_declarative_config(config, tls_config, key_resolver)
            .map(|provider| Box::new(provider.build()) as Box<dyn Provider>),
        ProviderEngine::Anthropic => {
            anthropic::from_declarative_config(config, tls_config, key_resolver)
                .map(|provider| Box::new(provider.build()) as Box<dyn Provider>)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    fn model_json() -> serde_json::Value {
        json!({
            "name": "test-model",
            "context_limit": 4096,
            "input_token_cost": null,
            "output_token_cost": null,
            "currency": null,
            "supports_cache_control": null,
            "reasoning": false
        })
    }

    #[test]
    fn provider_engine_deserializes_compatible_aliases() {
        let openai: DeclarativeProviderConfig = serde_json::from_value(json!({
            "name": "test-openai",
            "engine": "openai_compatible",
            "display_name": "Test OpenAI",
            "base_url": "http://localhost:1234",
            "models": [model_json()]
        }))
        .unwrap();
        assert_eq!(openai.engine, ProviderEngine::OpenAI);

        let anthropic: DeclarativeProviderConfig = serde_json::from_value(json!({
            "name": "test-anthropic",
            "engine": "anthropic_compatible",
            "display_name": "Test Anthropic",
            "base_url": "http://localhost:1234",
            "models": [model_json()]
        }))
        .unwrap();
        assert_eq!(anthropic.engine, ProviderEngine::Anthropic);

        let ollama: DeclarativeProviderConfig = serde_json::from_value(json!({
            "name": "test-ollama",
            "engine": "ollama_compatible",
            "display_name": "Test Ollama",
            "base_url": "http://localhost:11434",
            "models": [model_json()]
        }))
        .unwrap();
        assert_eq!(ollama.engine, ProviderEngine::Ollama);
    }

    #[test]
    fn groq_json_disables_thinking_preservation() {
        let config =
            deserialize_provider_config(crate::groq::JSON).expect("groq.json should parse");

        assert!(!config.preserves_thinking);
    }

    #[test]
    fn setup_metadata_rejects_unknown_fields() {
        let mut definition: serde_json::Value = serde_json::from_str(crate::groq::JSON).unwrap();
        definition["setup"]["description"] = json!("This field would be ignored");

        let error = deserialize_provider_config(&definition.to_string()).unwrap_err();

        assert!(error.to_string().contains("unknown field `description`"));
    }

    fn placeholder_var_names(template: &str) -> Vec<String> {
        template
            .split("${")
            .skip(1)
            .filter_map(|chunk| chunk.split_once('}'))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    fn validate_provider_id(id: &str) -> Result<()> {
        let mut chars = id.chars();
        let Some(first) = chars.next() else {
            anyhow::bail!("Invalid provider id: provider id cannot be empty");
        };

        if !(first.is_ascii_lowercase() || first.is_ascii_digit() || first == '_') {
            anyhow::bail!("Invalid provider id: {id}");
        }

        if chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
        {
            Ok(())
        } else {
            anyhow::bail!("Invalid provider id: {id}")
        }
    }

    #[test]
    fn expose_declarative_providers_enumerates_all_bundled_json_files() {
        let enumerated: HashSet<_> = fixed_provider_config_entries()
            .into_iter()
            .map(|(path, _)| path.to_string())
            .collect();
        let bundled: HashSet<_> = FIXED_PROVIDERS
            .files()
            .filter(|file| file.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .map(|file| {
                file.path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(enumerated, bundled);
    }

    #[test]
    fn all_bundled_providers_are_valid() {
        let mut seen_ids = HashSet::new();

        for (path, json) in fixed_provider_config_entries() {
            let config = deserialize_provider_config(json)
                .unwrap_or_else(|e| panic!("{path} failed to parse: {e}"));

            validate_provider_id(config.id())
                .unwrap_or_else(|e| panic!("{path} has an invalid provider id: {e}"));
            assert!(
                seen_ids.insert(config.id().to_string()),
                "{path} has a duplicate provider id: {}",
                config.id()
            );
            assert!(!config.base_url.is_empty(), "{path} has an empty base_url");

            if config.dynamic_models == Some(false) {
                assert!(
                    !config.models.is_empty(),
                    "{path} disables dynamic_models but lists no static models"
                );
            }

            let declared: HashSet<&str> = config
                .env_vars
                .iter()
                .flatten()
                .map(|v| v.name.as_str())
                .collect();
            let templates = std::iter::once(config.base_url.as_str())
                .chain(config.base_path.as_deref())
                .chain(
                    config
                        .headers
                        .iter()
                        .flat_map(|h| h.values())
                        .map(String::as_str),
                );
            for template in templates {
                for var in placeholder_var_names(template) {
                    assert!(
                        declared.contains(var.as_str()),
                        "{path} references ${{{var}}} but declares no matching env_var"
                    );
                }
            }
        }

        assert!(!seen_ids.is_empty(), "no bundled providers were found");
    }

    #[test]
    fn opencode_go_overrides_session_id_header() {
        let config = fixed_provider_configs()
            .expect("bundled providers should load")
            .into_iter()
            .find(|config| config.name == "opencode_go")
            .expect("opencode_go should be bundled");

        assert_eq!(
            config.session_id_header_override.as_deref(),
            Some("x-opencode-session")
        );
    }

    #[test]
    fn fixed_provider_configs_are_unresolved() {
        let configs = fixed_provider_configs().expect("bundled providers should load");
        let config = configs
            .iter()
            .find(|config| config.env_vars.is_some())
            .expect("at least one bundled provider should declare env_vars");

        assert!(
            config.base_url.contains("${"),
            "{} should keep base_url placeholders unresolved",
            config.id()
        );
    }

    #[test]
    fn from_json_defaults_openai_preserves_thinking_to_true() {
        let json = json!({
            "name": "test-provider",
            "engine": "openai",
            "display_name": "Test Provider",
            "base_url": "http://localhost:1234/v1/chat/completions",
            "models": [model_json()],
            "requires_auth": false,
            "dynamic_models": false
        })
        .to_string();

        let config = config_from_json(&json).unwrap();

        assert!(config.preserves_thinking);
    }

    #[test]
    fn from_json_preserves_explicit_openai_preserves_thinking_false() {
        let json = json!({
            "name": "test-provider",
            "engine": "openai",
            "display_name": "Test Provider",
            "base_url": "http://localhost:1234/v1/chat/completions",
            "models": [model_json()],
            "requires_auth": false,
            "dynamic_models": false,
            "preserves_thinking": false
        })
        .to_string();

        let config = config_from_json(&json).unwrap();

        assert!(!config.preserves_thinking);
    }

    #[test]
    fn from_json_expands_base_url_from_env_var_default() {
        let _guard = env_lock::lock_env([("TEST_PROVIDER_HOST", None::<&str>)]);
        let json = json!({
            "name": "test-provider",
            "engine": "openai",
            "display_name": "Test Provider",
            "base_url": "${TEST_PROVIDER_HOST}/v1/chat/completions",
            "models": [model_json()],
            "requires_auth": false,
            "dynamic_models": false,
            "env_vars": [{
                "name": "TEST_PROVIDER_HOST",
                "default": "http://localhost:1234"
            }]
        })
        .to_string();

        let provider = from_json(&json, None, EnvKeyResolver).unwrap();

        assert_eq!(provider.get_name(), "test-provider");
    }

    #[tokio::test]
    async fn from_json_ollama_returns_static_models_when_dynamic_models_false() {
        let json = json!({
            "name": "test-ollama",
            "engine": "ollama",
            "display_name": "Test Ollama",
            "base_url": "http://localhost:11434",
            "models": [model_json()],
            "requires_auth": false,
            "dynamic_models": false
        })
        .to_string();

        let provider = from_json(&json, None, EnvKeyResolver).unwrap();

        assert_eq!(
            provider.fetch_supported_models().await.unwrap(),
            vec!["test-model".to_string()]
        );
    }

    #[test]
    fn from_json_errors_when_required_env_var_is_missing() {
        let _guard = env_lock::lock_env([("TEST_PROVIDER_REQUIRED_HOST", None::<&str>)]);
        let json = json!({
            "name": "test-provider",
            "engine": "openai",
            "display_name": "Test Provider",
            "base_url": "${TEST_PROVIDER_REQUIRED_HOST}/v1/chat/completions",
            "models": [model_json()],
            "requires_auth": false,
            "dynamic_models": false,
            "env_vars": [{
                "name": "TEST_PROVIDER_REQUIRED_HOST",
                "required": true
            }]
        })
        .to_string();

        let err = match from_json(&json, None, EnvKeyResolver) {
            Ok(_) => panic!("expected missing required env var error"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("Required environment variable TEST_PROVIDER_REQUIRED_HOST is not set"));
    }
}
