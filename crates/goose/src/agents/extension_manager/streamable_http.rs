use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName};
use rmcp::model::{
    CallToolResult, ErrorCode, ErrorData, GetPromptResult, ProtocolVersion, ServerInfo,
    ServerNotification,
};
use rmcp::service::{ClientInitializeError, ServiceError};
use rmcp::transport::auth::{AuthClient, CredentialStore};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use rmcp::transport::{DynamicTransportError, IntoTransport, StreamableHttpClientTransport};
use rmcp::RoleClient;
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::super::extension::{ExtensionError, ExtensionResult};
use super::super::mcp_client::{ConnectContext, McpClient, McpClientTrait};
use super::super::tool_execution::ToolCallContext;
use crate::oauth::{oauth_flow, oauth_flow_with_challenge, StaticOAuthClientConfig};

/// Retry with OAuth for typed auth challenges and wrapped bare HTTP 401 responses.
fn is_oauth_auth_failure(err: &ClientInitializeError) -> bool {
    let ClientInitializeError::TransportError {
        error: DynamicTransportError { error, .. },
        ..
    } = err
    else {
        return false;
    };

    if let Some(http_err) = error.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
        return match http_err {
            StreamableHttpError::AuthRequired(_) | StreamableHttpError::InsufficientScope(_) => {
                true
            }
            StreamableHttpError::UnexpectedServerResponse(body) => {
                body.starts_with("HTTP 401") || body.starts_with("HTTP 403")
            }
            _ => false,
        };
    }

    #[cfg(unix)]
    if let Some(http_err) = error
        .downcast_ref::<StreamableHttpError<rmcp::transport::common::unix_socket::UnixSocketError>>(
        )
    {
        return match http_err {
            StreamableHttpError::AuthRequired(_) | StreamableHttpError::InsufficientScope(_) => {
                true
            }
            StreamableHttpError::UnexpectedServerResponse(body) => {
                body.starts_with("HTTP 401") || body.starts_with("HTTP 403")
            }
            _ => false,
        };
    }

    let message = error.to_string();
    message.contains("unexpected server response: HTTP 401")
        || message.contains("unexpected server response: HTTP 403")
        || message.contains("Auth required")
        || message.contains("Authorization required")
}

fn should_attempt_oauth_fallback(res: &Result<McpClient, ClientInitializeError>) -> bool {
    res.as_ref().err().is_some_and(is_oauth_auth_failure)
}

/// Extract the `WWW-Authenticate` challenge from a failed initialization, so
/// OAuth discovery can be seeded from the server's 401/403 response instead of
/// probing well-known locations.
fn auth_challenge_from_error(err: &ClientInitializeError) -> Option<String> {
    let ClientInitializeError::TransportError {
        error: DynamicTransportError { error, .. },
        ..
    } = err
    else {
        return None;
    };

    if let Some(http_err) = error.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
        return http_err.auth_challenge().map(str::to_string);
    }

    #[cfg(unix)]
    if let Some(http_err) = error
        .downcast_ref::<StreamableHttpError<rmcp::transport::common::unix_socket::UnixSocketError>>(
        )
    {
        return http_err.auth_challenge().map(str::to_string);
    }

    None
}

fn auth_challenge_from_result(res: &Result<McpClient, ClientInitializeError>) -> Option<String> {
    res.as_ref().err().and_then(auth_challenge_from_error)
}

/// Extract the `WWW-Authenticate` challenge from a post-initialization request
/// failure (401 auth required or 403 insufficient scope), so a step-up
/// authorization can be started reactively.
fn auth_challenge_from_service_error(err: &ServiceError) -> Option<String> {
    let ServiceError::TransportSend(DynamicTransportError { error, .. }) = err else {
        return None;
    };

    if let Some(http_err) = error.downcast_ref::<StreamableHttpError<reqwest::Error>>() {
        return http_err.auth_challenge().map(str::to_string);
    }

    #[cfg(unix)]
    if let Some(http_err) = error
        .downcast_ref::<StreamableHttpError<rmcp::transport::common::unix_socket::UnixSocketError>>(
        )
    {
        return http_err.auth_challenge().map(str::to_string);
    }

    None
}

async fn clear_credentials_on_post_refresh_auth_failure(
    credential_store: &dyn CredentialStore,
    name: &str,
    error: &ExtensionError,
) -> bool {
    let ExtensionError::InitializeError(err) = error else {
        return false;
    };

    if !is_oauth_auth_failure(err)
        || auth_challenge_from_error(err).is_some_and(|challenge| {
            challenge
                .to_ascii_lowercase()
                .contains("insufficient_scope")
        })
    {
        return false;
    }

    if let Err(e) = credential_store.clear().await {
        warn!(
            "[OAuth:{}] error clearing rejected credentials: {}",
            name, e
        );
    }
    true
}

pub(super) fn resolve_static_oauth_client(
    client_id: Option<&str>,
    client_secret_key: Option<&str>,
    scopes: &[String],
    envs: &HashMap<String, String>,
) -> Result<Option<StaticOAuthClientConfig>, Box<ExtensionError>> {
    let Some(client_id) = client_id else {
        if client_secret_key.is_some() {
            return Err(Box::new(ExtensionError::ConfigError(
                "client_secret_key requires client_id".to_string(),
            )));
        }
        if !scopes.is_empty() {
            return Err(Box::new(ExtensionError::ConfigError(
                "scopes requires client_id".to_string(),
            )));
        }
        return Ok(None);
    };

    let client_secret = match client_secret_key {
        Some(key) => Some(envs.get(key).cloned().ok_or_else(|| {
            Box::new(ExtensionError::ConfigError(format!(
                "Secret '{}' not found",
                key
            )))
        })?),
        None => None,
    };

    Ok(Some(StaticOAuthClientConfig {
        client_id: client_id.to_string(),
        client_secret,
        scopes: scopes.to_vec(),
    }))
}

const GOOSE_USER_AGENT: reqwest::header::HeaderValue =
    reqwest::header::HeaderValue::from_static(concat!("goose/", env!("CARGO_PKG_VERSION")));

// These return String rather than ExtensionError because clippy's
// result_large_err sizes the error by its largest variant; callers wrap the
// String back into ExtensionError::ConfigError.
fn header_map(headers: &HashMap<String, String>) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    map.insert(reqwest::header::USER_AGENT, GOOSE_USER_AGENT);
    for (key, value) in headers {
        map.insert(
            HeaderName::try_from(key).map_err(|_| format!("invalid header: {}", key))?,
            value
                .parse()
                .map_err(|_| format!("invalid header value: {}", key))?,
        );
    }
    Ok(map)
}

#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn http_client(
    headers: &HashMap<String, String>,
    timeout: Duration,
) -> Result<reqwest::Client, String> {
    #[allow(unused_mut)]
    let mut builder = reqwest::Client::builder().default_headers(header_map(headers)?);
    #[cfg(target_os = "linux")]
    {
        builder = builder.tcp_user_timeout(Some(timeout));
    }
    builder
        .build()
        .map_err(|_| "could not construct http client".to_string())
}

fn should_retry_legacy_after_empty_discover(
    result: &Result<McpClient, ClientInitializeError>,
    capabilities: &super::super::mcp_client::GooseMcpClientCapabilities,
) -> bool {
    capabilities.protocol_version.is_none()
        && result.as_ref().is_err_and(|error| {
            error.to_string().contains("empty sse stream")
                || matches!(
                    error,
                    ClientInitializeError::ConnectionClosed(context)
                        if context == "discover response"
                )
        })
}

// TODO: Remove this compatibility retry once rmcp handles empty discovery SSE
// responses upstream.
async fn connect_with_legacy_retry<T, E, A>(
    make_transport: impl Fn() -> T,
    ctx: ConnectContext,
) -> Result<McpClient, ClientInitializeError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + From<std::io::Error> + Send + Sync + 'static,
{
    let result = McpClient::connect(make_transport(), ctx.clone()).await;
    if !should_retry_legacy_after_empty_discover(&result, &ctx.capabilities) {
        return result;
    }
    let mut ctx = ctx;
    ctx.capabilities.protocol_version = Some(ProtocolVersion::V_2025_11_25);
    McpClient::connect(make_transport(), ctx).await
}

async fn connect_with_auth(
    auth_manager: rmcp::transport::AuthorizationManager,
    uri: &str,
    headers: &HashMap<String, String>,
    ctx: ConnectContext,
) -> ExtensionResult<McpClient> {
    let auth_client = AuthClient::new(
        http_client(headers, ctx.timeout).map_err(ExtensionError::ConfigError)?,
        auth_manager,
    );
    Ok(connect_with_legacy_retry(
        || {
            StreamableHttpClientTransport::with_client(
                auth_client.clone(),
                StreamableHttpClientTransportConfig::with_uri(uri),
            )
        },
        ctx,
    )
    .await?)
}

/// Connection parameters needed to re-establish an authorized streamable HTTP
/// client after a post-initialization auth challenge (401/403).
#[derive(Clone)]
pub(super) struct ConnectParams {
    pub(super) uri: String,
    pub(super) name: String,
    pub(super) headers: HashMap<String, String>,
    pub(super) static_oauth_client: Option<StaticOAuthClientConfig>,
    pub(super) ctx: ConnectContext,
}

/// Wraps a streamable HTTP `McpClient` and handles step-up authorization:
/// when a request fails with a 401/403 carrying a `WWW-Authenticate`
/// challenge after initialization succeeded, re-authorize using the challenge
/// (requesting the union of scopes), reconnect, and retry the request once.
struct OAuthStepUpClient {
    inner: tokio::sync::RwLock<McpClient>,
    server_info: Option<ServerInfo>,
    params: tokio::sync::RwLock<ConnectParams>,
    step_up_lock: tokio::sync::Mutex<()>,
    notification_subscribers: Arc<Mutex<Vec<mpsc::Sender<ServerNotification>>>>,
}

impl OAuthStepUpClient {
    async fn new(inner: McpClient, params: ConnectParams) -> Self {
        let server_info = inner.get_info().cloned();
        let notification_subscribers = Arc::new(Mutex::new(Vec::new()));
        Self::forward_notifications(&inner, notification_subscribers.clone()).await;
        Self {
            inner: tokio::sync::RwLock::new(inner),
            server_info,
            params: tokio::sync::RwLock::new(params),
            step_up_lock: tokio::sync::Mutex::new(()),
            notification_subscribers,
        }
    }

    async fn forward_notifications(
        client: &McpClient,
        subscribers: Arc<Mutex<Vec<mpsc::Sender<ServerNotification>>>>,
    ) {
        let mut receiver = client.subscribe().await;
        tokio::spawn(async move {
            while let Some(notification) = receiver.recv().await {
                let mut subscribers = subscribers.lock().await;
                subscribers.retain(|subscriber| subscriber.try_send(notification.clone()).is_ok());
            }
        });
    }

    async fn step_up_reconnect(
        &self,
        challenge: String,
    ) -> Result<(), crate::agents::mcp_client::Error> {
        let params = self.params.read().await;
        let auth_manager = oauth_flow_with_challenge(
            &params.uri,
            &params.name,
            params.static_oauth_client.as_ref(),
            Some(challenge),
        )
        .await
        .map_err(|e| {
            crate::agents::mcp_client::Error::McpError(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("step-up authorization failed: {e}"),
                None,
            ))
        })?;
        let client = connect_with_auth(
            auth_manager,
            &params.uri,
            &params.headers,
            params.ctx.clone(),
        )
        .await
        .map_err(|e| {
            crate::agents::mcp_client::Error::McpError(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("reconnect after step-up authorization failed: {e}"),
                None,
            ))
        })?;
        Self::forward_notifications(&client, self.notification_subscribers.clone()).await;
        *self.inner.write().await = client;
        Ok(())
    }

    /// Run `op` against the current client; on an auth challenge, re-authorize
    /// and retry once.
    async fn with_step_up_retry<T, F>(&self, op: F) -> Result<T, crate::agents::mcp_client::Error>
    where
        F: for<'a> Fn(
            &'a McpClient,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<T, crate::agents::mcp_client::Error>>
                    + Send
                    + 'a,
            >,
        >,
    {
        let first = {
            let client = self.inner.read().await;
            op(&client).await
        };
        match first {
            Err(err) => {
                if let Some(challenge) = auth_challenge_from_service_error(&err) {
                    let _step_up_guard = self.step_up_lock.lock().await;
                    let retry = {
                        let client = self.inner.read().await;
                        op(&client).await
                    };
                    match retry {
                        Ok(value) => Ok(value),
                        Err(retry_err)
                            if auth_challenge_from_service_error(&retry_err).is_some() =>
                        {
                            self.step_up_reconnect(challenge).await?;
                            let client = self.inner.read().await;
                            op(&client).await
                        }
                        Err(retry_err) => Err(retry_err),
                    }
                } else {
                    Err(err)
                }
            }
            ok => ok,
        }
    }
}

#[async_trait::async_trait]
impl McpClientTrait for OAuthStepUpClient {
    async fn list_tools(
        &self,
        session_id: &str,
        next_cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<rmcp::model::ListToolsResult, crate::agents::mcp_client::Error> {
        let session_id = session_id.to_string();
        self.with_step_up_retry(move |client| {
            let session_id = session_id.clone();
            let next_cursor = next_cursor.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move {
                client
                    .list_tools(&session_id, next_cursor, cancel_token)
                    .await
            })
        })
        .await
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<rmcp::model::JsonObject>,
        cancel_token: CancellationToken,
    ) -> Result<CallToolResult, crate::agents::mcp_client::Error> {
        let ctx = ctx.clone();
        let name = name.to_string();
        self.with_step_up_retry(move |client| {
            let ctx = ctx.clone();
            let name = name.clone();
            let arguments = arguments.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move { client.call_tool(&ctx, &name, arguments, cancel_token).await })
        })
        .await
    }

    fn get_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    async fn list_resources(
        &self,
        session_id: &str,
        next_cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<rmcp::model::ListResourcesResult, crate::agents::mcp_client::Error> {
        let session_id = session_id.to_string();
        self.with_step_up_retry(move |client| {
            let session_id = session_id.clone();
            let next_cursor = next_cursor.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move {
                client
                    .list_resources(&session_id, next_cursor, cancel_token)
                    .await
            })
        })
        .await
    }

    async fn read_resource(
        &self,
        session_id: &str,
        uri: &str,
        cancel_token: CancellationToken,
    ) -> Result<rmcp::model::ReadResourceResult, crate::agents::mcp_client::Error> {
        let session_id = session_id.to_string();
        let uri = uri.to_string();
        self.with_step_up_retry(move |client| {
            let session_id = session_id.clone();
            let uri = uri.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move { client.read_resource(&session_id, &uri, cancel_token).await })
        })
        .await
    }

    async fn list_prompts(
        &self,
        session_id: &str,
        next_cursor: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<rmcp::model::ListPromptsResult, crate::agents::mcp_client::Error> {
        let session_id = session_id.to_string();
        self.with_step_up_retry(move |client| {
            let session_id = session_id.clone();
            let next_cursor = next_cursor.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move {
                client
                    .list_prompts(&session_id, next_cursor, cancel_token)
                    .await
            })
        })
        .await
    }

    async fn get_prompt(
        &self,
        session_id: &str,
        name: &str,
        arguments: Value,
        cancel_token: CancellationToken,
    ) -> Result<GetPromptResult, crate::agents::mcp_client::Error> {
        let session_id = session_id.to_string();
        let name = name.to_string();
        self.with_step_up_retry(move |client| {
            let session_id = session_id.clone();
            let name = name.clone();
            let arguments = arguments.clone();
            let cancel_token = cancel_token.clone();
            Box::pin(async move {
                client
                    .get_prompt(&session_id, &name, arguments, cancel_token)
                    .await
            })
        })
        .await
    }

    async fn subscribe(&self) -> tokio::sync::mpsc::Receiver<rmcp::model::ServerNotification> {
        let (sender, receiver) = mpsc::channel(32);
        self.notification_subscribers.lock().await.push(sender);
        receiver
    }

    async fn get_moim(&self, session_id: &str) -> Option<String> {
        self.inner.read().await.get_moim(session_id).await
    }

    async fn update_working_dir(
        &self,
        new_dir: PathBuf,
    ) -> Result<(), crate::agents::mcp_client::Error> {
        self.params.write().await.ctx.working_dir = new_dir.clone();
        self.inner.read().await.update_working_dir(new_dir).await
    }
}

pub(super) async fn connect(
    params: ConnectParams,
    socket: Option<&str>,
    credential_store: Box<dyn CredentialStore>,
) -> ExtensionResult<Box<dyn McpClientTrait>> {
    #[cfg(unix)]
    if let Some(socket_path) = socket {
        return connect_over_unix_socket(&params, socket_path).await;
    }
    #[cfg(not(unix))]
    if socket.is_some() {
        return Err(ExtensionError::ConfigError(
            "Unix domain socket transport is not supported on this platform".to_string(),
        ));
    }

    let ConnectParams {
        uri,
        name,
        headers,
        static_oauth_client,
        ctx,
    } = &params;
    let http_client = http_client(headers, ctx.timeout).map_err(ExtensionError::ConfigError)?;

    // If we have stored OAuth credentials, try refreshing and connecting directly.
    // This avoids the unnecessary 401 → browser re-auth cycle on every new session.
    if credential_store.load().await.is_ok_and(|c| c.is_some()) {
        match oauth_flow(uri, name, static_oauth_client.as_ref()).await {
            Ok(auth_manager) => {
                match connect_with_auth(auth_manager, uri, headers, ctx.clone()).await {
                    Ok(client) => {
                        return Ok(Box::new(OAuthStepUpClient::new(client, params).await));
                    }
                    Err(error) => {
                        if !clear_credentials_on_post_refresh_auth_failure(
                            credential_store.as_ref(),
                            name,
                            &error,
                        )
                        .await
                        {
                            return Err(error);
                        }
                        warn!(
                            "[OAuth:{}] Refreshed token was rejected, falling back to browser auth",
                            name
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[OAuth:{}] Proactive refresh failed: {}, falling back to unauthenticated attempt",
                    name, e
                );
            }
        }
    }

    let client_res = connect_with_legacy_retry(
        || {
            StreamableHttpClientTransport::with_client(
                http_client.clone(),
                StreamableHttpClientTransportConfig::with_uri(uri.as_str()),
            )
        },
        ctx.clone(),
    )
    .await;

    if !should_attempt_oauth_fallback(&client_res) {
        return Ok(Box::new(OAuthStepUpClient::new(client_res?, params).await));
    }

    let challenge = auth_challenge_from_result(&client_res);
    match oauth_flow_with_challenge(uri, name, static_oauth_client.as_ref(), challenge).await {
        Ok(auth_manager) => {
            let client = connect_with_auth(auth_manager, uri, headers, ctx.clone()).await?;
            Ok(Box::new(OAuthStepUpClient::new(client, params).await))
        }
        Err(e) => {
            warn!(
                "[OAuth:{}] Browser authorization flow failed: {:#}",
                name, e
            );
            Ok(Box::new(client_res?))
        }
    }
}

#[cfg(unix)]
async fn connect_over_unix_socket(
    params: &ConnectParams,
    socket_path: &str,
) -> ExtensionResult<Box<dyn McpClientTrait>> {
    use rmcp::transport::UnixSocketHttpClient;

    let unix_client = UnixSocketHttpClient::new(socket_path, &params.uri);
    let custom_headers: HashMap<HeaderName, axum::http::HeaderValue> = header_map(&params.headers)
        .map_err(ExtensionError::ConfigError)?
        .into_iter()
        .filter_map(|(name, value)| name.map(|name| (name, value)))
        .collect();

    let client_res = connect_with_legacy_retry(
        || {
            StreamableHttpClientTransport::with_client(
                unix_client.clone(),
                StreamableHttpClientTransportConfig::with_uri(params.uri.as_str())
                    .custom_headers(custom_headers.clone()),
            )
        },
        params.ctx.clone(),
    )
    .await;

    if should_attempt_oauth_fallback(&client_res) {
        warn!(
            "Extension '{}' returned 401 over Unix domain socket transport; \
             OAuth is not supported for UDS connections",
            params.name,
        );
    }
    Ok(Box::new(client_res?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_required_manager::ActionRequiredManager;
    use crate::agents::mcp_client::GooseMcpClientCapabilities;
    use rmcp::transport::auth::InMemoryCredentialStore;
    use std::sync::Weak;
    use tempfile::tempdir;

    fn test_ctx(working_dir: &std::path::Path) -> ConnectContext {
        ConnectContext {
            timeout: Duration::from_secs(5),
            provider: Arc::new(Mutex::new(None)),
            client_name: "goose-test".to_string(),
            capabilities: GooseMcpClientCapabilities {
                mcpui: false,
                host_info: None,
                elicitation_handler: None,
                protocol_version: None,
            },
            working_dir: working_dir.to_path_buf(),
            docker_container: None,
            action_required: Arc::new(ActionRequiredManager::new()),
            extension_manager: Weak::new(),
        }
    }

    fn test_params(
        uri: &str,
        headers: HashMap<String, String>,
        working_dir: &std::path::Path,
    ) -> ConnectParams {
        ConnectParams {
            uri: uri.to_string(),
            name: "test-ext".to_string(),
            headers,
            static_oauth_client: None,
            ctx: test_ctx(working_dir),
        }
    }

    fn transport_err(error: Box<dyn std::error::Error + Send + Sync>) -> ClientInitializeError {
        ClientInitializeError::TransportError {
            error: rmcp::transport::DynamicTransportError::from_parts(
                "test",
                std::any::TypeId::of::<()>(),
                error,
            ),
            context: "test context".into(),
        }
    }

    fn streamable_err(
        e: rmcp::transport::streamable_http_client::StreamableHttpError<reqwest::Error>,
    ) -> ClientInitializeError {
        transport_err(Box::new(e))
    }

    #[test]
    fn test_oauth_fallback_on_typed_auth_required() {
        let err = streamable_err(
            rmcp::transport::streamable_http_client::StreamableHttpError::AuthRequired(
                rmcp::transport::streamable_http_client::AuthRequiredError::new(
                    "Bearer realm=\"test\"".to_string(),
                ),
            ),
        );
        assert!(should_attempt_oauth_fallback(&Err(err)));
    }

    #[test]
    fn test_oauth_fallback_on_unexpected_response_http_401_prefix() {
        let err = streamable_err(
            rmcp::transport::streamable_http_client::StreamableHttpError::UnexpectedServerResponse(
                std::borrow::Cow::Borrowed("HTTP 401 Unauthorized"),
            ),
        );
        assert!(should_attempt_oauth_fallback(&Err(err)));
    }

    #[tokio::test]
    async fn test_post_refresh_auth_failure_clears_credentials() {
        use rmcp::transport::auth::{OAuthTokenResponse, StoredCredentials};

        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "rejected-token",
            "token_type": "bearer",
        }))
        .expect("valid fake token JSON");
        let store = InMemoryCredentialStore::new();
        store
            .save(StoredCredentials::new(
                "test-client".to_string(),
                Some(token_response),
                vec![],
                None,
            ))
            .await
            .unwrap();

        let err = streamable_err(
            rmcp::transport::streamable_http_client::StreamableHttpError::AuthRequired(
                rmcp::transport::streamable_http_client::AuthRequiredError::new(
                    "Bearer error=\"invalid_token\"".to_string(),
                ),
            ),
        );
        let error = ExtensionError::InitializeError(err);

        assert!(clear_credentials_on_post_refresh_auth_failure(&store, "test-ext", &error).await);
        assert!(store.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_invalid_header_name_returns_config_error() {
        let mut headers = HashMap::new();
        headers.insert("bad header name".to_string(), "value".to_string());

        let temp_dir = tempdir().unwrap();

        let result = connect(
            test_params("http://localhost:1", headers, temp_dir.path()),
            None,
            Box::new(InMemoryCredentialStore::new()),
        )
        .await;

        let Err(ExtensionError::ConfigError(msg)) = result else {
            panic!("expected ConfigError, got a different result");
        };
        assert!(
            msg.contains("invalid header"),
            "unexpected error message: {msg}"
        );
    }

    #[tokio::test]
    async fn test_invalid_header_value_returns_config_error() {
        let mut headers = HashMap::new();
        headers.insert("x-valid-name".to_string(), "bad\r\nvalue".to_string());

        let temp_dir = tempdir().unwrap();

        let result = connect(
            test_params("http://localhost:1", headers, temp_dir.path()),
            None,
            Box::new(InMemoryCredentialStore::new()),
        )
        .await;

        let Err(ExtensionError::ConfigError(msg)) = result else {
            panic!("expected ConfigError, got a different result");
        };
        assert!(
            msg.contains("invalid header value"),
            "unexpected error message: {msg}"
        );
    }

    #[tokio::test]
    async fn test_custom_headers_forwarded_to_http_extension() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), "test-secret-123".to_string());

        let temp_dir = tempdir().unwrap();

        // The MCP handshake will fail against the stub server. We only care that
        // the outgoing HTTP request carried the custom header.
        let _ = connect(
            test_params(&mock_server.uri(), headers, temp_dir.path()),
            None,
            Box::new(InMemoryCredentialStore::new()),
        )
        .await;

        let received = mock_server.received_requests().await.unwrap();
        assert!(
            !received.is_empty(),
            "expected at least one HTTP request to reach the mock server"
        );
        let header_found = received.iter().any(|req| {
            req.headers
                .get("x-api-key")
                .map(|v| v == "test-secret-123")
                .unwrap_or(false)
        });
        assert!(
            header_found,
            "custom header x-api-key was not forwarded to the extension server"
        );
    }

    /// Directly exercises `connect_with_auth`, which is the code path fixed by
    /// the PR (custom headers were dropped when the OAuth connection path was
    /// taken).  Uses a pre-seeded `InMemoryCredentialStore` with a fake,
    /// non-expiring token so `get_access_token()` returns immediately without
    /// touching any OAuth endpoints or the system keychain.
    #[tokio::test]
    async fn test_custom_headers_forwarded_oauth_path() {
        use rmcp::transport::auth::{OAuthTokenResponse, StoredCredentials};
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let mut headers = HashMap::new();
        headers.insert("x-api-key".to_string(), "test-secret-oauth".to_string());

        // Build a fake, non-expiring token. token_received_at=None skips the
        // expiry check, so get_access_token() returns without any network call.
        let token_response: OAuthTokenResponse = serde_json::from_value(serde_json::json!({
            "access_token": "fake-test-token",
            "token_type": "bearer",
        }))
        .expect("valid fake token JSON");
        let creds = StoredCredentials::new(
            "test-client".to_string(),
            Some(token_response),
            vec![],
            None,
        );
        let store = InMemoryCredentialStore::new();
        store.save(creds).await.unwrap();

        let mut auth_manager = rmcp::transport::AuthorizationManager::new(mock_server.uri())
            .await
            .expect("AuthorizationManager::new should not make network calls");
        auth_manager.set_credential_store(store);

        let temp_dir = tempdir().unwrap();

        // connect_with_auth will fail (mock server isn't an MCP server) but we
        // only care that the outgoing request carried the custom header.
        let _ = connect_with_auth(
            auth_manager,
            &mock_server.uri(),
            &headers,
            test_ctx(temp_dir.path()),
        )
        .await;

        let received = mock_server.received_requests().await.unwrap();
        assert!(
            !received.is_empty(),
            "expected at least one HTTP request to reach the mock server"
        );
        let header_found = received.iter().any(|req| {
            req.headers
                .get("x-api-key")
                .map(|v| v == "test-secret-oauth")
                .unwrap_or(false)
        });
        assert!(
            header_found,
            "custom header x-api-key was not forwarded through the OAuth connection path"
        );
    }

    mod static_oauth_client {
        use super::*;

        #[test]
        fn absent_client_id_yields_no_static_client() {
            let resolved = resolve_static_oauth_client(None, None, &[], &HashMap::new()).unwrap();

            assert_eq!(resolved, None);
        }

        #[test]
        fn client_id_without_secret_resolves_public_client() {
            let resolved = resolve_static_oauth_client(
                Some("registered-client"),
                None,
                &["scope.read".to_string()],
                &HashMap::new(),
            )
            .unwrap()
            .unwrap();

            assert_eq!(resolved.client_id, "registered-client");
            assert_eq!(resolved.client_secret, None);
            assert_eq!(resolved.scopes, vec!["scope.read"]);
        }

        #[test]
        fn client_secret_resolves_from_envs() {
            let envs =
                HashMap::from([("OAUTH_CLIENT_SECRET".to_string(), "env-secret".to_string())]);

            let resolved = resolve_static_oauth_client(
                Some("registered-client"),
                Some("OAUTH_CLIENT_SECRET"),
                &[],
                &envs,
            )
            .unwrap()
            .unwrap();

            assert_eq!(resolved.client_secret.as_deref(), Some("env-secret"));
        }

        #[test]
        fn client_secret_key_without_client_id_is_rejected() {
            let error = resolve_static_oauth_client(
                None,
                Some("OAUTH_CLIENT_SECRET"),
                &[],
                &HashMap::new(),
            )
            .unwrap_err();

            assert!(error
                .to_string()
                .contains("client_secret_key requires client_id"));
        }

        #[test]
        fn scopes_without_client_id_are_rejected() {
            let error = resolve_static_oauth_client(
                None,
                None,
                &["scope.read".to_string()],
                &HashMap::new(),
            )
            .unwrap_err();

            assert!(error.to_string().contains("scopes requires client_id"));
        }

        #[test]
        fn missing_client_secret_key_is_an_error() {
            let error = resolve_static_oauth_client(
                Some("registered-client"),
                Some("MISSING_KEY"),
                &[],
                &HashMap::new(),
            )
            .unwrap_err();

            assert!(error.to_string().contains("MISSING_KEY"));
        }
    }
}
