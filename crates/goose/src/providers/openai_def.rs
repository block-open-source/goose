use anyhow::Result;
use futures::future::BoxFuture;
use goose_providers::base::ProviderDescriptor;
use std::collections::HashMap;

use crate::config::declarative_providers::DeclarativeProviderConfig;
use crate::config::Config;
use crate::providers::base::{ProviderDef, DEFAULT_PROVIDER_TIMEOUT_SECS};
use crate::providers::command_auth::CommandAuthProvider;
use crate::providers::custom_provider_config::ConfigKeyResolver;
use crate::session_context::{
    session_id_request_builder, session_id_request_builder_with_header_override,
};
use goose_providers::api_client::{ApiClient, AuthMethod};
use goose_providers::openai::{
    parse_custom_headers, parse_openai_base_url, OpenAiProvider, OpenAiProviderBuilder,
    OPEN_AI_DEFAULT_BASE_PATH, OPEN_AI_VERSIONLESS_BASE_PATH,
};

pub struct OpenAiProviderDef;

impl ProviderDescriptor for OpenAiProviderDef {
    fn metadata() -> goose_providers::base::ProviderMetadata {
        OpenAiProvider::metadata().with_setup(
            crate::providers::catalog::ProviderSetupMetadata::new(
                crate::providers::catalog::ProviderSetupCategory::Model,
                crate::providers::catalog::ProviderSetupMethod::ConfigFields,
                crate::providers::catalog::ProviderSetupGroup::Default,
            )
            .with_docs_url("https://platform.openai.com/api-keys")
            .with_field(
                "OPENAI_API_KEY",
                "API Key",
                Some("Paste your API key"),
                None,
            ),
        )
    }
}

impl ProviderDef for OpenAiProviderDef {
    type Provider = OpenAiProvider;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(from_env(tls_config))
    }
}

pub async fn from_env(
    tls_config: Option<goose_providers::api_client::TlsConfig>,
) -> Result<OpenAiProvider> {
    let config = crate::config::Config::global();

    // Resolve host and base_path.
    //
    // Priority (highest first):
    //   1. OPENAI_HOST env var — session override (deprecated but still
    //      honoured so that `OPENAI_HOST=… goose` keeps working)
    //   2. OPENAI_BASE_URL (env or config) — ecosystem-standard
    //   3. OPENAI_HOST from config file — persisted by `goose configure`
    //   4. Default "https://api.openai.com"
    //
    // OPENAI_BASE_URL is parsed into host + query params + a flag
    // indicating whether the URL included a /v1 path segment.  When /v1
    // is present the default base_path is "v1/chat/completions";
    // otherwise "chat/completions" to match the OpenAI SDK convention.
    //
    // OPENAI_BASE_PATH always wins when set explicitly.
    let parsed = resolve_base_url(config)?;

    // When the host was derived from OPENAI_BASE_URL, read
    // OPENAI_BASE_PATH from env only so that the desktop UI's persisted
    // default ("v1/chat/completions") doesn't shadow the versionless
    // path.  When the host came from OPENAI_HOST (env or config), read
    // from config too — Docker Model Runner and similar setups persist a
    // custom base_path that must be honoured.
    let default_bp = || {
        if parsed.has_v1 {
            OPEN_AI_DEFAULT_BASE_PATH.to_string()
        } else {
            OPEN_AI_VERSIONLESS_BASE_PATH.to_string()
        }
    };
    let base_path: String = if parsed.from_base_url {
        std::env::var("OPENAI_BASE_PATH").unwrap_or_else(|_| default_bp())
    } else {
        config
            .get_param("OPENAI_BASE_PATH")
            .unwrap_or_else(|_| default_bp())
    };

    let is_openai = is_direct_openai_host(&parsed.host);
    let secrets = config
        .get_secrets("OPENAI_API_KEY", &["OPENAI_CUSTOM_HEADERS"])
        .unwrap_or_default();
    let api_key: Option<String> = secrets.get("OPENAI_API_KEY").cloned();
    let custom_headers: Option<HashMap<String, String>> = secrets
        .get("OPENAI_CUSTOM_HEADERS")
        .cloned()
        .map(parse_custom_headers);

    let organization: Option<String> = config.get_param("OPENAI_ORGANIZATION").ok();
    let project: Option<String> = config.get_param("OPENAI_PROJECT").ok();
    let timeout_secs: u64 = config
        .get_param("OPENAI_TIMEOUT")
        .unwrap_or(DEFAULT_PROVIDER_TIMEOUT_SECS);

    let auth = match api_key {
        Some(key) if !key.is_empty() => AuthMethod::BearerToken(key),
        _ => AuthMethod::NoAuth,
    };
    let mut api_client = ApiClient::with_timeout_and_tls(
        parsed.host,
        auth,
        std::time::Duration::from_secs(timeout_secs),
        tls_config,
    )?
    .with_request_builder(session_id_request_builder());

    if !parsed.query_params.is_empty() {
        api_client = api_client.with_query(parsed.query_params);
    }

    if let Some(org) = &organization {
        api_client = api_client.with_header("OpenAI-Organization", org)?;
    }

    if let Some(project) = &project {
        api_client = api_client.with_header("OpenAI-Project", project)?;
    }

    if let Some(headers) = &custom_headers {
        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, value) in headers {
            let header_name = reqwest::header::HeaderName::from_bytes(key.as_bytes())?;
            let header_value = reqwest::header::HeaderValue::from_str(value)?;
            header_map.insert(header_name, header_value);
        }
        api_client = api_client.with_headers(header_map)?;
    }

    let provider = OpenAiProviderBuilder::new(api_client)
        .base_path(base_path)
        .organization(organization)
        .project(project)
        .custom_headers(custom_headers)
        .preserve_thinking_context(!is_openai)
        .build();

    // TODO(jack): replace this
    // provider.probe_context_limit_if_unset(&mut model).await;

    Ok(provider)
}

/// Resolve the API key from a declarative provider config.
///
/// Returns `Some(key)` if a key is found, `None` if the key is optional/missing,
/// or an error if the key is required but missing/unreadable.
///
/// The `get_secret` closure is used to look up the secret by key name. This allows
/// testing without depending on `Config::global()`.
pub fn resolve_api_key(
    config: &DeclarativeProviderConfig,
    get_secret: &dyn Fn(&str) -> Result<String, crate::config::ConfigError>,
) -> Result<Option<String>> {
    if config.api_key_env.is_empty() {
        return Ok(None);
    }

    match get_secret(&config.api_key_env) {
        Ok(key) => Ok(Some(key)),
        Err(e) => {
            use crate::config::ConfigError;
            match e {
                ConfigError::NotFound(_) => {
                    if config.requires_auth {
                        anyhow::bail!(
                            "Required API key {} is not set. Configure it via `goose configure` or set the {} environment variable.",
                            config.api_key_env,
                            config.api_key_env
                        );
                    }
                    Ok(None)
                }
                other => {
                    if config.requires_auth {
                        anyhow::bail!("Failed to read {}: {}", config.api_key_env, other);
                    } else {
                        tracing::warn!(
                            "Failed to read optional API key {}: {}. Proceeding without authentication.",
                            config.api_key_env,
                            other
                        );
                        Ok(None)
                    }
                }
            }
        }
    }
}

pub fn from_custom_config(
    config: DeclarativeProviderConfig,
    tls_config: Option<goose_providers::api_client::TlsConfig>,
) -> Result<OpenAiProvider> {
    let auth_override = config.auth.clone();
    let request_builder = session_id_request_builder_with_header_override(
        config.session_id_header_override.as_deref(),
    )?;
    goose_providers::openai::from_declarative_config(
        config,
        tls_config,
        ConfigKeyResolver::new(Config::global()),
    )
    .map(|builder| {
        builder
            .map_api_client(|api_client| {
                let api_client = api_client.with_request_builder(request_builder);
                match auth_override {
                    Some(auth_config) => api_client.with_auth(AuthMethod::Custom(Box::new(
                        CommandAuthProvider::new(&auth_config, "Authorization", "Bearer "),
                    ))),
                    None => api_client,
                }
            })
            .build()
    })
}

/// Components extracted from an `OPENAI_BASE_URL` value.
struct ParsedBaseUrl {
    /// The host (scheme + authority + any path prefix before `/v1`).
    pub(crate) host: String,
    /// Query parameters to forward on every request.
    pub(crate) query_params: Vec<(String, String)>,
    /// Whether the URL path ended with `/v1`.
    pub(crate) has_v1: bool,
    /// `true` when the host was derived from `OPENAI_BASE_URL`.
    /// Controls whether `OPENAI_BASE_PATH` is read from env only
    /// (to avoid persisted desktop defaults shadowing URL-derived paths)
    /// or from config too (to honour Docker Model Runner setups).
    pub(crate) from_base_url: bool,
}

fn parse_base_url(raw_url: &str) -> Result<ParsedBaseUrl> {
    let (host, query_params, has_v1) = parse_openai_base_url(raw_url)?;
    Ok(ParsedBaseUrl {
        host,
        query_params,
        has_v1,
        from_base_url: true,
    })
}

/// Resolve the effective host from environment and config.
///
/// Priority (highest first):
///   1. OPENAI_HOST env var — session override (deprecated but still honoured)
///   2. OPENAI_BASE_URL (env or config) — ecosystem-standard
///   3. OPENAI_HOST from config file — persisted by `goose configure`
///   4. Default "https://api.openai.com"
fn resolve_base_url(config: &crate::config::Config) -> Result<ParsedBaseUrl> {
    if let Ok(h) = std::env::var("OPENAI_HOST") {
        return Ok(ParsedBaseUrl {
            host: h,
            query_params: vec![],
            has_v1: true,
            from_base_url: false,
        });
    }

    if let Some(raw_url) = config
        .get_param::<String>("OPENAI_BASE_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return parse_base_url(&raw_url);
    }

    let h: String = config
        .get_param("OPENAI_HOST")
        .unwrap_or_else(|_| "https://api.openai.com".to_string());
    Ok(ParsedBaseUrl {
        host: h,
        query_params: vec![],
        has_v1: true,
        from_base_url: false,
    })
}

/// Whether `host` points at OpenAI directly.
///
/// Compares the hostname exactly to avoid false positives (e.g.
/// `https://api.openai.com.local:8000` or proxy paths containing
/// `api.openai.com`).
fn is_direct_openai_host(host: &str) -> bool {
    url::Url::parse(host)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .map(|h| h == "api.openai.com" || h.ends_with(".api.openai.com"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_base_url_strips_v1_from_standard_openai_url() {
        let r = parse_base_url("https://api.openai.com/v1").unwrap();
        assert_eq!(r.host, "https://api.openai.com");
        assert!(r.query_params.is_empty());
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_preserves_prefix_before_v1() {
        let r = parse_base_url("https://gateway.example.com/openai/v1").unwrap();
        assert_eq!(r.host, "https://gateway.example.com/openai");
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_handles_no_path() {
        let r = parse_base_url("https://api.openai.com").unwrap();
        assert_eq!(r.host, "https://api.openai.com");
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_handles_trailing_slash() {
        let r = parse_base_url("https://api.openai.com/v1/").unwrap();
        assert_eq!(r.host, "https://api.openai.com");
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_preserves_port() {
        let r = parse_base_url("https://localhost:8080/v1").unwrap();
        assert_eq!(r.host, "https://localhost:8080");
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_preserves_non_v1_path() {
        let r = parse_base_url("https://example.com/custom/api").unwrap();
        assert_eq!(r.host, "https://example.com/custom/api");
        assert!(!r.has_v1);
    }

    #[test]
    fn is_direct_openai_host_matches_only_openai() {
        assert!(is_direct_openai_host("https://api.openai.com"));
        assert!(is_direct_openai_host("https://api.openai.com/v1"));
        assert!(is_direct_openai_host("https://eu.api.openai.com"));
        assert!(!is_direct_openai_host("https://api.openai.com.local:8000"));
        assert!(!is_direct_openai_host("https://localhost:1234"));
        assert!(!is_direct_openai_host("https://router.huggingface.co/v1"));
    }

    #[test]
    fn parse_base_url_preserves_query_params() {
        let r = parse_base_url("https://gw.example.com/v1?api-version=2024-02-01").unwrap();
        assert_eq!(r.host, "https://gw.example.com");
        assert_eq!(
            r.query_params,
            vec![("api-version".to_string(), "2024-02-01".to_string())]
        );
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_preserves_multiple_query_params() {
        let r = parse_base_url("https://example.com/v1?key=val&foo=bar").unwrap();
        assert_eq!(r.query_params.len(), 2);
        assert_eq!(r.query_params[0], ("key".to_string(), "val".to_string()));
        assert_eq!(r.query_params[1], ("foo".to_string(), "bar".to_string()));
    }

    #[test]
    fn parse_base_url_preserves_credentials() {
        let r = parse_base_url("https://user:pass@gateway.example.com/v1").unwrap();
        assert_eq!(r.host, "https://user:pass@gateway.example.com");
        assert!(r.has_v1);
    }

    #[test]
    fn parse_base_url_rejects_empty_string() {
        assert!(parse_base_url("").is_err());
    }

    #[test]
    fn parse_base_url_rejects_whitespace_only() {
        assert!(parse_base_url("  ").is_err());
    }
}
