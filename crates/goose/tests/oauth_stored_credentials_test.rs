use goose::oauth::{oauth_flow, GooseCredentialStore, StaticOAuthClientConfig};
use rmcp::transport::auth::{CredentialStore, OAuthTokenResponse, StoredCredentials};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn token_response(access_token: &str, refresh_token: &str) -> OAuthTokenResponse {
    serde_json::from_value(serde_json::json!({
        "access_token": access_token,
        "token_type": "bearer",
        "expires_in": 3600,
        "refresh_token": refresh_token,
    }))
    .expect("valid token response JSON")
}

fn expired_credentials() -> StoredCredentials {
    StoredCredentials::new(
        "test-client".to_string(),
        Some(token_response(
            "expired-access-token",
            "expired-refresh-token",
        )),
        vec![],
        Some(now_epoch() - 7200),
    )
}

fn fresh_credentials(access_token: &str, refresh_token: &str) -> StoredCredentials {
    StoredCredentials::new(
        "test-client".to_string(),
        Some(token_response(access_token, refresh_token)),
        vec![],
        Some(now_epoch()),
    )
}

async fn seed_expired_credentials(store: &GooseCredentialStore) {
    store.clear().await.unwrap();
    store
        .save_with_requested_scopes(expired_credentials(), Some(vec![]))
        .unwrap();
}

fn static_client() -> StaticOAuthClientConfig {
    StaticOAuthClientConfig {
        client_id: "test-client".to_string(),
        client_secret: None,
        scopes: vec![],
    }
}

async fn mount_authorization_server_metadata(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": server.uri(),
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
        })))
        .mount(server)
        .await;
}

/// The browser leg of the flow fails fast: `GOOSE_OAUTH_AUTOMATIC_CALLBACK`
/// makes goose request the authorization URL directly, and a 500 response
/// (no redirect) aborts the flow without opening a browser.
async fn mount_failing_authorization_endpoint(server: &MockServer, expected_hits: u64) {
    Mock::given(method("GET"))
        .and(path("/authorize"))
        .respond_with(ResponseTemplate::new(500))
        .expect(expected_hits)
        .mount(server)
        .await;
}

async fn refresh_token_of(store: &GooseCredentialStore) -> Option<String> {
    store
        .load()
        .await
        .unwrap()
        .and_then(|credentials| credentials.token_response)
        .and_then(|response| {
            use oauth2::TokenResponse;
            response.refresh_token().map(|t| t.secret().to_string())
        })
}

#[tokio::test]
async fn oauth_refresh_failures_preserve_credentials_correctly() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let root = temp_dir.path().to_string_lossy().to_string();
    let _guard = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
        ("GOOSE_ADDITIONAL_CONFIG_FILES", None::<&str>),
        ("GOOSE_OAUTH_CALLBACK_TIMEOUT", Some("5")),
        ("GOOSE_OAUTH_AUTOMATIC_CALLBACK", Some("1")),
        ("GOOSE_MCP_OAUTH_CLIENT_ID", None::<&str>),
        ("GOOSE_MCP_OAUTH_CLIENT_SECRET", None::<&str>),
    ]);

    let name = "test-oauth-ext".to_string();
    let store = GooseCredentialStore::new(name.clone());
    let server = MockServer::start().await;
    let mcp_url = format!("{}/mcp", server.uri());
    let client = static_client();

    // Scenario 1: a transient refresh failure (server 5xx) must keep the
    // stored credentials so the next session can refresh again, instead of
    // wiping them and forcing browser re-authentication on every session.
    seed_expired_credentials(&store).await;
    mount_authorization_server_metadata(&server).await;
    mount_failing_authorization_endpoint(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream unavailable"))
        .expect(1)
        .mount(&server)
        .await;

    let result = oauth_flow(&mcp_url, &name, Some(&client)).await;
    assert!(
        result.is_err(),
        "browser auth cannot complete in this scenario"
    );
    assert_eq!(
        refresh_token_of(&store).await.as_deref(),
        Some("expired-refresh-token"),
        "transient refresh failures must not clear stored credentials"
    );
    server.reset().await;

    // Scenario 2: a successful refresh stores the refreshed credentials and
    // never touches the authorization endpoint.
    seed_expired_credentials(&store).await;
    mount_authorization_server_metadata(&server).await;
    mount_failing_authorization_endpoint(&server, 0).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "refreshed-access-token",
            "token_type": "bearer",
            "expires_in": 3600,
            "refresh_token": "refreshed-refresh-token",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let manager = oauth_flow(&mcp_url, &name, Some(&client))
        .await
        .expect("refresh must succeed");
    server.verify().await;
    assert!(manager.get_access_token().await.is_ok());
    assert_eq!(
        refresh_token_of(&store).await.as_deref(),
        Some("refreshed-refresh-token"),
        "refreshed credentials must be persisted"
    );
    server.reset().await;

    // Scenario 3: a definitive rejection (invalid_grant) with no concurrent
    // rotation still clears the stored credentials.
    seed_expired_credentials(&store).await;
    mount_authorization_server_metadata(&server).await;
    mount_failing_authorization_endpoint(&server, 1).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": "invalid_grant"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = oauth_flow(&mcp_url, &name, Some(&client)).await;
    assert!(result.is_err());
    assert!(
        store.load().await.unwrap().is_none(),
        "a rejected refresh token must be cleared"
    );
    server.reset().await;

    // Scenario 4: a definitive rejection after ANOTHER goose process already
    // rotated the stored refresh token must reuse the replacement instead of
    // clobbering it (desktop + CLI share one credential store).
    seed_expired_credentials(&store).await;
    mount_authorization_server_metadata(&server).await;
    mount_failing_authorization_endpoint(&server, 0).await;
    let rotation_store = Arc::new(store.clone());
    let rotating_rejection = move |_: &wiremock::Request| {
        rotation_store
            .save_with_requested_scopes(
                fresh_credentials("rotated-access-token", "rotated-refresh-token"),
                Some(vec![]),
            )
            .unwrap();
        ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": "invalid_grant"}))
    };
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(rotating_rejection)
        .expect(1)
        .mount(&server)
        .await;

    let manager = oauth_flow(&mcp_url, &name, Some(&client))
        .await
        .expect("rotated credentials from another process must be reused");
    server.verify().await;
    assert_eq!(
        refresh_token_of(&store).await.as_deref(),
        Some("rotated-refresh-token"),
        "credentials refreshed by another process must survive a lost refresh race"
    );
    drop(manager);
}
