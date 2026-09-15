mod persist;

pub use persist::GooseCredentialStore;

use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use fs2::FileExt;
use minijinja::{context, Environment};
use oauth2::{Scope, TokenResponse};
use rmcp::transport::auth::{
    AuthError, AuthorizationRequest, CredentialStore, OAuthClientConfig, OAuthState,
    OAuthTokenResponse, StoredCredentials, WWWAuthenticateParams,
};
use rmcp::transport::AuthorizationManager;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{oneshot, Mutex};
use tracing::warn;

use crate::config::paths::Paths;

const CALLBACK_TEMPLATE: &str = include_str!("oauth_callback.html");
const CLIENT_METADATA_URL: &str = "https://goose-docs.ai/oauth/client-metadata.json";
const DEFAULT_OAUTH_CALLBACK_TIMEOUT_SECS: u64 = 300;
const OAUTH_CALLBACK_TIMEOUT_ENV: &str = "GOOSE_OAUTH_CALLBACK_TIMEOUT_SECONDS";

/// Pre-registered OAuth client supplied by a probe script, for servers whose
/// authorization server supports neither Dynamic Client Registration nor
/// Client ID Metadata Documents.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub client_metadata_url: Option<String>,
}

#[derive(Clone)]
struct AppState {
    callback_receiver: Arc<Mutex<Option<oneshot::Sender<String>>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
    iss: Option<String>,
}

fn resolve_oauth_callback_timeout(value: Option<&str>) -> Duration {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(DEFAULT_OAUTH_CALLBACK_TIMEOUT_SECS))
}

fn oauth_callback_timeout() -> Duration {
    let timeout = std::env::var(OAUTH_CALLBACK_TIMEOUT_ENV).ok();
    resolve_oauth_callback_timeout(timeout.as_deref())
}

fn render_oauth_callback(name: &str) -> String {
    Environment::new()
        .render_named_str(
            "oauth_callback.html",
            CALLBACK_TEMPLATE,
            context! { name => name },
        )
        .expect("failed to render OAuth callback")
}

fn announce_authorization_url(name: &str, authorization_url: &str) {
    warn!(
        "[OAuth:{}] If the browser did not open, authorize manually at: {}",
        name, authorization_url
    );
    eprintln!(
        "If the browser did not open, authorize {} at:\n  {}",
        name, authorization_url
    );
}

async fn complete_automatic_authorization(
    authorization_url: &str,
    redirect_uri: &str,
) -> Result<Option<String>, anyhow::Error> {
    if std::env::var_os("GOOSE_OAUTH_AUTOMATIC_CALLBACK").is_none() {
        return Ok(None);
    }

    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(authorization_url)
        .send()
        .await?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| anyhow::anyhow!("authorization response did not include Location"))?
        .to_str()?;
    let callback_url = url::Url::parse(location)?;
    let expected_redirect = url::Url::parse(redirect_uri)?;
    if callback_url.scheme() != expected_redirect.scheme()
        || callback_url.host_str() != expected_redirect.host_str()
        || callback_url.port_or_known_default() != expected_redirect.port_or_known_default()
        || callback_url.path() != expected_redirect.path()
    {
        anyhow::bail!("authorization response redirected to an unexpected callback URI");
    }
    Ok(Some(callback_url.to_string()))
}

async fn wait_for_callback(
    callback_receiver: oneshot::Receiver<String>,
    timeout_duration: Duration,
    name: &str,
    authorization_url: &str,
) -> Result<String, anyhow::Error> {
    match tokio::time::timeout(timeout_duration, callback_receiver).await {
        Ok(Ok(callback_url)) => Ok(callback_url),
        Ok(Err(e)) => Err(anyhow::anyhow!(
            "OAuth authorization for {} ended before the callback was received: {}",
            name,
            e
        )),
        Err(_) => {
            let message = format!(
                "OAuth authorization for {} timed out waiting for the local callback. \
                 Start the OAuth flow again and open this URL manually if the browser does not open: {}",
                name, authorization_url
            );
            warn!("[OAuth:{}] {}", name, message);
            Err(anyhow::anyhow!(message))
        }
    }
}

/// OAuth client credentials registered with the authorization server out of
/// band, for servers whose authorization server supports neither Dynamic
/// Client Registration nor Client ID Metadata Documents.
#[derive(Clone, Debug, PartialEq)]
pub struct StaticOAuthClientConfig {
    pub client_id: String,
    /// Secret paired with the client ID. Optional: public clients using PKCE
    /// have no secret.
    pub client_secret: Option<String>,
    /// Scopes to request. When empty, scopes are selected from server
    /// metadata, which may be broader than the extension needs.
    pub scopes: Vec<String>,
}

/// Pre-registered client supplied through the environment, used by tools that
/// drive the flow without an extension config (`goose mcp-probe`, conformance
/// driver).
fn env_static_oauth_client() -> Option<StaticOAuthClientConfig> {
    Some(StaticOAuthClientConfig {
        client_id: std::env::var("GOOSE_MCP_OAUTH_CLIENT_ID").ok()?,
        client_secret: std::env::var("GOOSE_MCP_OAUTH_CLIENT_SECRET").ok(),
        scopes: Vec::new(),
    })
}

fn client_metadata_url() -> String {
    std::env::var("GOOSE_MCP_OAUTH_CLIENT_METADATA_URL")
        .unwrap_or_else(|_| CLIENT_METADATA_URL.to_string())
}

fn scope_set(scopes: &[String]) -> BTreeSet<&str> {
    scopes.iter().map(String::as_str).collect()
}

fn configured_scopes_changed(
    static_client: Option<&StaticOAuthClientConfig>,
    previous_requested_scopes: Option<&[String]>,
    granted_scopes: &[String],
) -> bool {
    let Some(client) = static_client else {
        return previous_requested_scopes.is_some();
    };

    match previous_requested_scopes {
        Some(previous) => scope_set(previous) != scope_set(&client.scopes),
        None => !scope_set(&client.scopes).is_subset(&scope_set(granted_scopes)),
    }
}

fn configured_client_changed(
    static_client: Option<&StaticOAuthClientConfig>,
    stored_client_id: &str,
) -> bool {
    static_client.is_some_and(|client| client.client_id != stored_client_id)
}

fn resolve_refreshed_granted_scopes(
    token_scopes: Option<Vec<String>>,
    previous_granted_scopes: &[String],
) -> Vec<String> {
    token_scopes.unwrap_or_else(|| previous_granted_scopes.to_vec())
}

const REFRESH_BUFFER_SECS: u64 = 30;
const MAX_OAUTH_LOCK_STEM_LEN: usize = 200;

fn sanitize_oauth_lock_name(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    let hash: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() || stem.chars().all(|c| c == '_') {
        stem = "unnamed".to_string();
    }
    let max_stem = MAX_OAUTH_LOCK_STEM_LEN.saturating_sub(hash.len() + 1);
    if stem.len() > max_stem {
        stem.truncate(max_stem);
    }
    format!("{stem}_{hash}")
}

fn challenged_scopes(challenge: &str, mcp_server_url: &str) -> Vec<String> {
    let Ok(base_url) = url::Url::parse(mcp_server_url) else {
        return Vec::new();
    };
    WWWAuthenticateParams::parse(challenge, &base_url)
        .scope
        .map(|scope| {
            scope
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn stored_access_token(stored: &StoredCredentials) -> Option<&str> {
    stored
        .token_response
        .as_ref()
        .map(|token| token.access_token().secret().as_str())
}

fn stored_grant_satisfies_challenge(
    stored: &StoredCredentials,
    challenge: &str,
    mcp_server_url: &str,
) -> bool {
    if stored.token_response.is_none() {
        return false;
    }
    let needed = challenged_scopes(challenge, mcp_server_url);
    if needed.is_empty() {
        return true;
    }
    let granted = scope_set(&stored.granted_scopes);
    needed.iter().all(|scope| granted.contains(scope.as_str()))
}

fn challenge_can_reuse_stored_grant(
    rejected_access_token: Option<&str>,
    stored: Option<&StoredCredentials>,
    challenge: Option<&str>,
    mcp_server_url: &str,
) -> bool {
    let Some(challenge) = challenge else {
        return true;
    };
    stored.is_some_and(|stored| {
        stored_access_token(stored) != rejected_access_token
            && stored_grant_satisfies_challenge(stored, challenge, mcp_server_url)
    })
}

fn oauth_flow_lock_path(name: &str) -> PathBuf {
    Paths::config_dir()
        .join("oauth")
        .join(format!("{}.lock", sanitize_oauth_lock_name(name)))
}

fn lock_oauth_flow(name: &str) -> Result<File, anyhow::Error> {
    let path = oauth_flow_lock_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    file.lock_exclusive()
        .map_err(|e| anyhow::anyhow!("Failed to lock OAuth flow for {name}: {e}"))?;
    Ok(file)
}

async fn acquire_oauth_flow_lock(name: &str) -> Result<File, anyhow::Error> {
    let lock_name = name.to_string();
    tokio::task::spawn_blocking(move || lock_oauth_flow(&lock_name))
        .await
        .map_err(|e| anyhow::anyhow!("OAuth flow lock task failed: {e}"))?
}

fn access_token_needs_refresh(stored_credentials: &StoredCredentials) -> bool {
    let Some(token_response) = stored_credentials.token_response.as_ref() else {
        return true;
    };
    let (Some(expires_in), Some(received_at)) = (
        token_response.expires_in(),
        stored_credentials.token_received_at,
    ) else {
        return false;
    };
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(received_at);
    expires_in.as_secs().saturating_sub(elapsed) < REFRESH_BUFFER_SECS
}

fn configure_static_client(
    auth_manager: &mut AuthorizationManager,
    static_client: Option<&StaticOAuthClientConfig>,
    redirect_uri: &str,
) -> Result<(), AuthError> {
    let Some(client) = static_client else {
        return Ok(());
    };

    let mut config = OAuthClientConfig::new(client.client_id.clone(), redirect_uri.to_string());
    if let Some(secret) = &client.client_secret {
        config = config.with_client_secret(secret.clone());
    }
    auth_manager.configure_client(config)
}

fn restore_omitted_scopes(
    token_response: &mut OAuthTokenResponse,
    granted_scopes: &[String],
) -> bool {
    if token_response.scopes().is_some() || granted_scopes.is_empty() {
        return false;
    }

    token_response.set_scopes(Some(
        granted_scopes.iter().cloned().map(Scope::new).collect(),
    ));
    true
}

fn build_authorization_request(
    redirect_uri: String,
    static_client: Option<&StaticOAuthClientConfig>,
    challenge: Option<String>,
    mcp_server_url: &str,
    previously_granted_scopes: &[String],
) -> AuthorizationRequest {
    let mut request = AuthorizationRequest::new(redirect_uri).with_client_name("goose");
    match static_client {
        Some(client) => {
            request = request.with_preregistered_client(client.client_id.clone());
            if let Some(secret) = &client.client_secret {
                request = request.with_client_secret(secret.clone());
            }
            if !client.scopes.is_empty() {
                request = request.with_scopes(client.scopes.clone());
            }
        }
        None => {
            request = request.with_client_metadata_url(client_metadata_url());
        }
    }

    if let Some(challenge) = challenge {
        // SEP-2350: a re-authorization triggered by a scope challenge requests
        // the union of previously-granted scopes and the newly challenged
        // scopes. The fresh AuthorizationManager has no scope memory, so seed
        // the union from the stored grant.
        let mut scopes = previously_granted_scopes.to_vec();
        scopes.extend(
            static_client
                .into_iter()
                .flat_map(|client| client.scopes.iter().cloned()),
        );
        scopes.extend(challenged_scopes(&challenge, mcp_server_url));
        let mut seen = BTreeSet::new();
        scopes.retain(|scope| seen.insert(scope.clone()));
        if !scopes.is_empty() {
            request = request.with_scopes(scopes);
        }
        request = request.with_challenge(challenge);
    }

    request
}

pub async fn oauth_flow(
    mcp_server_url: &String,
    name: &String,
    static_client: Option<&StaticOAuthClientConfig>,
) -> Result<AuthorizationManager, anyhow::Error> {
    oauth_flow_with_challenge(mcp_server_url, name, static_client, None, None).await
}

pub async fn oauth_flow_with_challenge(
    mcp_server_url: &String,
    name: &String,
    static_client: Option<&StaticOAuthClientConfig>,
    challenge: Option<String>,
    rejected_access_token: Option<&str>,
) -> Result<AuthorizationManager, anyhow::Error> {
    let env_client = env_static_oauth_client();
    let static_client = static_client.or(env_client.as_ref());
    let credential_store = GooseCredentialStore::new(name.clone());
    let mut auth_manager = AuthorizationManager::new(mcp_server_url).await?;
    // Serialize refresh and browser auth per extension across processes so a
    // rotating refresh token cannot be spent twice and then wipe the winner.
    let _oauth_lock = acquire_oauth_flow_lock(name).await?;
    credential_store.invalidate_cache();
    auth_manager.set_credential_store(credential_store.clone());

    let stored_credentials = credential_store.load().await?;
    let previous_requested_scopes = credential_store.load_requested_scopes()?;
    let previously_granted_scopes = stored_credentials
        .as_ref()
        .map(|stored| stored.granted_scopes.clone())
        .unwrap_or_default();

    // After the lock, reuse a stored grant only when another process wrote a
    // new token that already covers this challenge. Compare against the token
    // the caller actually presented, not whatever the shared store currently
    // holds (another in-process client may have already saved a successor).
    let challenge_already_satisfied = challenge_can_reuse_stored_grant(
        rejected_access_token,
        stored_credentials.as_ref(),
        challenge.as_deref(),
        mcp_server_url,
    );

    if auth_manager.initialize_from_store().await? && challenge_already_satisfied {
        let stored_credentials = stored_credentials
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OAuth credentials disappeared during startup"))?;
        let previous_granted_scopes = stored_credentials.granted_scopes.as_slice();
        let scopes_changed = configured_scopes_changed(
            static_client,
            previous_requested_scopes.as_deref(),
            previous_granted_scopes,
        );
        let client_changed =
            configured_client_changed(static_client, &stored_credentials.client_id);

        if !scopes_changed && !client_changed {
            // initialize_from_store configures the client from the stored
            // client_id alone; a confidential client must present its secret
            // at the token endpoint for the refresh to succeed.
            configure_static_client(&mut auth_manager, static_client, mcp_server_url)?;
            if !access_token_needs_refresh(stored_credentials) {
                return Ok(auth_manager);
            }
            match auth_manager.refresh_token().await {
                Ok(mut token_response) => {
                    let restored_omitted_scopes =
                        restore_omitted_scopes(&mut token_response, previous_granted_scopes);
                    let mut refreshed_credentials =
                        credential_store.load().await?.ok_or_else(|| {
                            anyhow::anyhow!("OAuth refresh did not persist credentials")
                        })?;
                    let refreshed_client_id = refreshed_credentials.client_id.clone();
                    refreshed_credentials.token_response = Some(token_response.clone());
                    refreshed_credentials.granted_scopes = resolve_refreshed_granted_scopes(
                        token_response
                            .scopes()
                            .map(|scopes| scopes.iter().map(|scope| scope.to_string()).collect()),
                        previous_granted_scopes,
                    );
                    let requested_scopes = static_client
                        .map(|client| client.scopes.clone())
                        .or(previous_requested_scopes);
                    credential_store
                        .save_with_requested_scopes(refreshed_credentials, requested_scopes)?;

                    if restored_omitted_scopes {
                        let mut oauth_state = OAuthState::new(mcp_server_url, None).await?;
                        oauth_state
                            .set_credentials(&refreshed_client_id, token_response)
                            .await?;
                        let mut restored_manager =
                            oauth_state.into_authorization_manager().ok_or_else(|| {
                                anyhow::anyhow!("Failed to restore OAuth authorization manager")
                            })?;
                        configure_static_client(
                            &mut restored_manager,
                            static_client,
                            mcp_server_url,
                        )?;
                        restored_manager.set_credential_store(credential_store);
                        return Ok(restored_manager);
                    }
                    return Ok(auth_manager);
                }
                Err(e) => {
                    warn!(
                        "[OAuth:{}] Token refresh failed: {} - clearing stored credentials and falling back to browser auth",
                        name, e
                    );
                }
            }
        }

        if let Err(e) = credential_store.clear().await {
            warn!("[OAuth:{}] error clearing bad credentials: {}", name, e);
        }
    }

    let (callback_sender, callback_receiver) = oneshot::channel::<String>();
    let app_state = AppState {
        callback_receiver: Arc::new(Mutex::new(Some(callback_sender))),
    };
    let rendered = render_oauth_callback(name);
    let handler = move |Query(params): Query<CallbackParams>, State(state): State<AppState>| {
        let rendered = rendered.clone();
        async move {
            if let Some(sender) = state.callback_receiver.lock().await.take() {
                let query = serde_urlencoded::to_string([
                    ("code", params.code.as_str()),
                    ("state", params.state.as_str()),
                ])
                .unwrap_or_default();
                let issuer = params
                    .iss
                    .as_deref()
                    .map(|iss| format!("&iss={}", urlencoding::encode(iss)))
                    .unwrap_or_default();
                let _ = sender.send(format!("http://callback/oauth_callback?{query}{issuer}"));
            }
            Html(rendered)
        }
    };
    let app = Router::new()
        .route("/oauth_callback", get(handler))
        .with_state(app_state);

    let port = std::env::var("GOOSE_OAUTH_CALLBACK_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    let used_addr = listener.local_addr()?;
    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("Callback server error: {}", e);
        }
    });

    let mut oauth_state = OAuthState::new(mcp_server_url, None).await?;
    let redirect_uri = format!("http://127.0.0.1:{}/oauth_callback", used_addr.port());
    oauth_state
        .start_authorization(build_authorization_request(
            redirect_uri.clone(),
            static_client,
            challenge,
            mcp_server_url,
            &previously_granted_scopes,
        ))
        .await?;

    let authorization_url = oauth_state.get_authorization_url().await?;
    let callback_url = async {
        if let Some(callback_url) =
            complete_automatic_authorization(authorization_url.as_str(), &redirect_uri).await?
        {
            Ok(callback_url)
        } else {
            announce_authorization_url(name, authorization_url.as_str());
            if let Err(e) = webbrowser::open(authorization_url.as_str()) {
                warn!(
                    "[OAuth:{}] Failed to open browser automatically: {}",
                    name, e
                );
            }
            wait_for_callback(
                callback_receiver,
                oauth_callback_timeout(),
                name,
                authorization_url.as_str(),
            )
            .await
        }
    }
    .await;
    server_handle.abort();
    oauth_state.handle_callback_url(&callback_url?).await?;

    let (client_id, token_response) = oauth_state.get_credentials().await?;
    let mut auth_manager = oauth_state
        .into_authorization_manager()
        .ok_or_else(|| anyhow::anyhow!("Failed to get authorization manager"))?;

    let granted_scopes = match token_response.as_ref().and_then(|tr| tr.scopes()) {
        Some(scopes) => scopes.iter().map(|scope| scope.to_string()).collect(),
        None => auth_manager.get_current_scopes().await,
    };
    credential_store.save_with_requested_scopes(
        StoredCredentials::new(
            client_id,
            token_response,
            granted_scopes,
            Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0),
            ),
        ),
        static_client.map(|client| client.scopes.clone()),
    )?;

    auth_manager.set_credential_store(credential_store);

    Ok(auth_manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_oauth_callback_timeout_uses_default_for_missing_or_invalid_values() {
        assert_eq!(
            resolve_oauth_callback_timeout(None),
            Duration::from_secs(DEFAULT_OAUTH_CALLBACK_TIMEOUT_SECS)
        );
        assert_eq!(
            resolve_oauth_callback_timeout(Some("not-a-number")),
            Duration::from_secs(DEFAULT_OAUTH_CALLBACK_TIMEOUT_SECS)
        );
        assert_eq!(
            resolve_oauth_callback_timeout(Some("0")),
            Duration::from_secs(DEFAULT_OAUTH_CALLBACK_TIMEOUT_SECS)
        );
    }

    #[test]
    fn resolve_oauth_callback_timeout_uses_positive_values() {
        assert_eq!(
            resolve_oauth_callback_timeout(Some("42")),
            Duration::from_secs(42)
        );
    }

    #[test]
    fn oauth_callback_escapes_extension_name() {
        let payload = r#"<script>alert("xss")</script>&"#;
        let rendered = render_oauth_callback(payload);

        assert!(!rendered.contains(payload));
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(rendered.contains("&amp;"));
    }

    #[test]
    fn oauth_callback_preserves_plain_extension_name() {
        let rendered = render_oauth_callback("Example MCP");

        assert!(rendered.contains("Example MCP OAuth Success"));
        assert!(rendered.contains(">Example MCP</span>"));
    }

    #[tokio::test]
    async fn wait_for_callback_returns_received_callback_url() {
        let (sender, receiver) = oneshot::channel();
        let expected = "http://callback/oauth_callback?code=auth-code&state=csrf-state";
        sender.send(expected.to_string()).unwrap();

        let callback_url = wait_for_callback(
            receiver,
            Duration::from_secs(1),
            "test-server",
            "https://auth.example/authorize",
        )
        .await
        .unwrap();

        assert_eq!(callback_url, expected);
    }

    #[test]
    fn callback_params_capture_rfc_9207_issuer() {
        let uri: axum::http::Uri =
            "http://127.0.0.1/oauth_callback?code=auth-code&state=csrf-state&iss=https%3A%2F%2Fauth.example%2Fidp"
                .parse()
                .unwrap();

        let Query(params) = Query::<CallbackParams>::try_from_uri(&uri).unwrap();

        assert_eq!(params.iss.as_deref(), Some("https://auth.example/idp"));
    }

    #[test]
    fn callback_params_accept_missing_issuer() {
        let uri: axum::http::Uri =
            "http://127.0.0.1/oauth_callback?code=auth-code&state=csrf-state"
                .parse()
                .unwrap();

        let Query(params) = Query::<CallbackParams>::try_from_uri(&uri).unwrap();

        assert_eq!(params.iss, None);
    }

    #[test]
    fn unchanged_scope_request_preserves_a_narrowed_grant() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: None,
            scopes: vec!["scope.read".to_string(), "scope.write".to_string()],
        };

        assert!(!configured_scopes_changed(
            Some(&static_client),
            Some(&["scope.read".to_string(), "scope.write".to_string()]),
            &["scope.read".to_string()],
        ));
    }

    #[test]
    fn changed_scope_request_requires_reauthorization() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: None,
            scopes: vec!["scope.read".to_string(), "scope.write".to_string()],
        };

        assert!(configured_scopes_changed(
            Some(&static_client),
            Some(&["scope.read".to_string()]),
            &["scope.read".to_string()],
        ));
    }

    #[test]
    fn changed_static_client_requires_reauthorization() {
        let static_client = StaticOAuthClientConfig {
            client_id: "new-client".to_string(),
            client_secret: None,
            scopes: vec![],
        };

        assert!(configured_client_changed(
            Some(&static_client),
            "old-client"
        ));
        assert!(!configured_client_changed(
            Some(&static_client),
            "new-client"
        ));
        assert!(!configured_client_changed(None, "old-client"));
    }

    #[test]
    fn legacy_grant_reauthorizes_once_only_when_scopes_are_missing() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: None,
            scopes: vec!["scope.read".to_string(), "scope.write".to_string()],
        };

        assert!(configured_scopes_changed(
            Some(&static_client),
            None,
            &["scope.read".to_string()],
        ));
        assert!(!configured_scopes_changed(
            Some(&static_client),
            None,
            &["scope.read".to_string(), "scope.write".to_string()],
        ));
    }

    #[test]
    fn removing_static_client_configuration_requires_reauthorization() {
        assert!(configured_scopes_changed(
            None,
            Some(&["scope.read".to_string()]),
            &["scope.read".to_string()],
        ));
        assert!(!configured_scopes_changed(
            None,
            None,
            &["scope.read".to_string()],
        ));
    }

    #[test]
    fn omitted_refresh_scope_preserves_the_previous_grant() {
        use oauth2::{basic::BasicTokenType, AccessToken};
        use rmcp::transport::auth::VendorExtraTokenFields;

        let previous = vec!["scope.read".to_string()];
        let mut token_response = OAuthTokenResponse::new(
            AccessToken::new("access-token".to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );

        assert_eq!(resolve_refreshed_granted_scopes(None, &previous), previous);
        assert!(restore_omitted_scopes(&mut token_response, &previous));
        assert_eq!(
            token_response
                .scopes()
                .unwrap()
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>(),
            vec!["scope.read"]
        );
        assert!(!restore_omitted_scopes(&mut token_response, &previous));
        assert_eq!(
            resolve_refreshed_granted_scopes(Some(vec!["scope.other".to_string()]), &previous),
            vec!["scope.other"]
        );
    }

    #[test]
    fn authorization_request_uses_client_metadata_url_without_static_client() {
        let request = build_authorization_request(
            "http://127.0.0.1:1234/oauth_callback".to_string(),
            None,
            None,
            "https://mcp.example",
            &[],
        );

        assert_eq!(request.client_id, None);
        assert_eq!(request.client_secret, None);
        assert_eq!(
            request.client_metadata_url.as_deref(),
            Some(CLIENT_METADATA_URL)
        );
        assert!(request.scopes.is_empty());
    }

    #[test]
    fn authorization_request_prefers_static_client_over_client_metadata_url() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: Some("registered-secret".to_string()),
            scopes: vec!["scope.read".to_string(), "scope.write".to_string()],
        };

        let request = build_authorization_request(
            "http://127.0.0.1:1234/oauth_callback".to_string(),
            Some(&static_client),
            None,
            "https://mcp.example",
            &[],
        );

        assert_eq!(request.client_id.as_deref(), Some("registered-client"));
        assert_eq!(request.client_secret.as_deref(), Some("registered-secret"));
        assert_eq!(request.client_metadata_url, None);
        assert_eq!(request.scopes, vec!["scope.read", "scope.write"]);
    }

    #[test]
    fn authorization_request_omits_secret_and_scopes_for_public_static_client() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: None,
            scopes: vec![],
        };

        let request = build_authorization_request(
            "http://127.0.0.1:1234/oauth_callback".to_string(),
            Some(&static_client),
            None,
            "https://mcp.example",
            &[],
        );

        assert_eq!(request.client_id.as_deref(), Some("registered-client"));
        assert_eq!(request.client_secret, None);
        assert_eq!(request.client_metadata_url, None);
        assert!(request.scopes.is_empty());
    }

    #[test]
    fn challenge_request_asks_for_the_union_of_granted_and_challenged_scopes() {
        let request = build_authorization_request(
            "http://127.0.0.1:1234/oauth_callback".to_string(),
            None,
            Some(
                r#"Bearer error="insufficient_scope", scope="scope.write scope.admin""#.to_string(),
            ),
            "https://mcp.example",
            &["scope.read".to_string(), "scope.write".to_string()],
        );

        assert_eq!(
            request.scopes,
            vec!["scope.read", "scope.write", "scope.admin"]
        );
        assert!(request.challenge.is_some());
    }

    #[test]
    fn challenge_request_keeps_static_client_scopes() {
        let static_client = StaticOAuthClientConfig {
            client_id: "registered-client".to_string(),
            client_secret: None,
            scopes: vec!["scope.read".to_string()],
        };

        let request = build_authorization_request(
            "http://127.0.0.1:1234/oauth_callback".to_string(),
            Some(&static_client),
            Some(r#"Bearer error="insufficient_scope", scope="scope.write""#.to_string()),
            "https://mcp.example",
            &[],
        );

        assert_eq!(request.client_id.as_deref(), Some("registered-client"));
        assert_eq!(request.scopes, vec!["scope.read", "scope.write"]);
    }

    #[test]
    fn long_lived_token_without_refresh_token_is_not_refreshed() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "mcp_long_lived",
            "token_type": "bearer",
            "expires_in": 31_536_000,
        }))
        .unwrap();
        let stored = StoredCredentials::new(
            "client-id".to_string(),
            Some(token_response.clone()),
            vec![],
            Some(now),
        );

        assert!(token_response.refresh_token().is_none());
        assert!(
            !access_token_needs_refresh(&stored),
            "a token valid for another year must be used as-is instead of forcing browser re-auth"
        );

        let expired = StoredCredentials::new(
            "client-id".to_string(),
            Some(token_response),
            vec![],
            Some(now - 31_536_000),
        );
        assert!(
            access_token_needs_refresh(&expired),
            "an expired token must still take the refresh path"
        );
    }

    #[test]
    fn missing_expiry_metadata_does_not_force_refresh() {
        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "mcp_no_expiry",
            "token_type": "bearer",
        }))
        .unwrap();
        let stored =
            StoredCredentials::new("client-id".to_string(), Some(token_response), vec![], None);

        assert!(
            !access_token_needs_refresh(&stored),
            "legacy credentials without expiry metadata must be used as-is"
        );
    }

    #[test]
    fn oauth_lock_name_replaces_unsafe_characters() {
        let pi = sanitize_oauth_lock_name("Pi Swisssync");
        assert!(pi.starts_with("Pi_Swisssync_"));
        assert_eq!(pi.len(), "Pi_Swisssync".len() + 1 + 16);

        let passwd = sanitize_oauth_lock_name("../etc/passwd");
        assert!(passwd.starts_with("___etc_passwd_"));

        assert!(sanitize_oauth_lock_name("").starts_with("unnamed_"));
        assert!(sanitize_oauth_lock_name("///").starts_with("unnamed_"));
        assert_ne!(
            sanitize_oauth_lock_name(""),
            sanitize_oauth_lock_name("///"),
            "empty and punctuation-only names must not share a lock"
        );

        let composio = sanitize_oauth_lock_name("Composio Connect");
        assert!(composio.starts_with("Composio_Connect_"));

        assert_ne!(
            sanitize_oauth_lock_name("Pi Swisssync"),
            sanitize_oauth_lock_name("Pi_Swisssync"),
            "names that only differ by separators must not share a lock"
        );

        let long_name = "a".repeat(MAX_OAUTH_LOCK_STEM_LEN + 50);
        let long = sanitize_oauth_lock_name(&long_name);
        assert_eq!(long.len(), MAX_OAUTH_LOCK_STEM_LEN);
        assert_ne!(
            sanitize_oauth_lock_name(&format!("{long_name}x")),
            long,
            "distinct long names must not collide"
        );
    }

    #[test]
    fn stored_grant_satisfies_empty_or_covered_challenge() {
        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "mcp_token",
            "token_type": "bearer",
            "expires_in": 3600,
        }))
        .unwrap();
        let stored = StoredCredentials::new(
            "client-id".to_string(),
            Some(token_response),
            vec!["scope.read".to_string(), "scope.write".to_string()],
            Some(1),
        );

        assert!(stored_grant_satisfies_challenge(
            &stored,
            r#"Bearer error="invalid_token""#,
            "https://mcp.example",
        ));
        assert!(stored_grant_satisfies_challenge(
            &stored,
            r#"Bearer error="insufficient_scope", scope="scope.read""#,
            "https://mcp.example",
        ));
        assert!(!stored_grant_satisfies_challenge(
            &stored,
            r#"Bearer error="insufficient_scope", scope="scope.admin""#,
            "https://mcp.example",
        ));
    }

    #[test]
    fn missing_token_does_not_satisfy_a_challenge() {
        let stored = StoredCredentials::new("client-id".to_string(), None, vec![], None);
        assert!(!stored_grant_satisfies_challenge(
            &stored,
            r#"Bearer error="invalid_token""#,
            "https://mcp.example",
        ));
    }

    fn stored_with_token(access_token: &str, scopes: &[&str]) -> StoredCredentials {
        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": access_token,
            "token_type": "bearer",
            "expires_in": 3600,
        }))
        .unwrap();
        StoredCredentials::new(
            "client-id".to_string(),
            Some(token_response),
            scopes.iter().map(|scope| (*scope).to_string()).collect(),
            Some(1),
        )
    }

    #[test]
    fn challenge_reuse_requires_a_new_token() {
        let rejected = stored_with_token("old-token", &["scope.read"]);
        let refreshed = stored_with_token("new-token", &["scope.read"]);
        let invalid_token = r#"Bearer error="invalid_token""#;
        let extra_scope = r#"Bearer error="insufficient_scope", scope="scope.admin""#;

        assert!(
            !challenge_can_reuse_stored_grant(
                Some("old-token"),
                Some(&rejected),
                Some(invalid_token),
                "https://mcp.example",
            ),
            "the token that triggered the 401 must not be reused"
        );
        assert!(
            challenge_can_reuse_stored_grant(
                Some("old-token"),
                Some(&refreshed),
                Some(invalid_token),
                "https://mcp.example",
            ),
            "a waiter that still holds the rejected token must reuse the successor grant"
        );
        assert!(
            !challenge_can_reuse_stored_grant(
                Some("old-token"),
                Some(&refreshed),
                Some(extra_scope),
                "https://mcp.example",
            ),
            "a new token still missing the challenged scope must not skip browser auth"
        );
        assert!(challenge_can_reuse_stored_grant(
            None,
            Some(&refreshed),
            None,
            "https://mcp.example",
        ));
    }

    #[test]
    fn oauth_flow_lock_path_is_stable_per_original_name() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        let _guard = env_lock::lock_env([("GOOSE_PATH_ROOT", Some(root))]);

        let path = oauth_flow_lock_path("Pi Swisssync");
        assert_eq!(
            path.parent(),
            Some(temp.path().join("config").join("oauth").as_path())
        );
        assert!(path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("Pi_Swisssync_") && name.ends_with(".lock")));
        assert_eq!(
            oauth_flow_lock_path("Pi Swisssync"),
            oauth_flow_lock_path("Pi Swisssync")
        );
        assert_ne!(
            oauth_flow_lock_path("Pi Swisssync"),
            oauth_flow_lock_path("Pi_Swisssync")
        );
    }

    #[test]
    fn oauth_flow_lock_is_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        let _guard = env_lock::lock_env([("GOOSE_PATH_ROOT", Some(root))]);

        let held = lock_oauth_flow("Pi Swisssync").unwrap();
        let path = oauth_flow_lock_path("Pi Swisssync");
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            contender.try_lock_exclusive().is_err(),
            "a second process must wait for the OAuth flow lock"
        );
        drop(held);
        assert!(contender.try_lock_exclusive().is_ok());
    }

    #[tokio::test]
    async fn wait_for_callback_times_out_with_authorization_url() {
        let (_sender, receiver) = oneshot::channel();

        let error = wait_for_callback(
            receiver,
            Duration::from_millis(1),
            "test-server",
            "https://auth.example/authorize",
        )
        .await
        .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("test-server"));
        assert!(message.contains("timed out"));
        assert!(message.contains("https://auth.example/authorize"));
    }
}
