#![recursion_limit = "256"]
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, Response, StatusCode};
use axum::Router;
use goose::acp::server::AcpBuiltinSelection;
use goose::acp::server_factory::{AcpServer, AcpServerFactoryConfig};
use goose::acp::transport::{create_acp_router, create_router};
use goose::agents::GoosePlatform;
use tower::ServiceExt;

const SECRET: &str = "test-secret-token";

fn test_router(require_token: bool, dir: &tempfile::TempDir) -> Router {
    test_router_with_origins(require_token, dir, Vec::new())
}

fn test_acp_router(dir: &tempfile::TempDir) -> Router {
    let server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins: AcpBuiltinSelection::default(),
        data_dir: dir.path().join("data"),
        config_dir: dir.path().join("config"),
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        session_cwd: None,
        enable_scheduler: false,
    }));
    create_acp_router(server)
}

fn test_authenticated_acp_router(dir: &tempfile::TempDir) -> Router {
    let server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins: AcpBuiltinSelection::default(),
        data_dir: dir.path().join("data"),
        config_dir: dir.path().join("config"),
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        session_cwd: None,
        enable_scheduler: false,
    }));
    create_router(server, SECRET.to_string(), true, Vec::new())
}

fn test_router_with_origins(
    require_token: bool,
    dir: &tempfile::TempDir,
    additional_allowed_origins: Vec<HeaderValue>,
) -> Router {
    let server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins: AcpBuiltinSelection::default(),
        data_dir: dir.path().join("data"),
        config_dir: dir.path().join("config"),
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        session_cwd: None,
        enable_scheduler: false,
    }));
    create_router(
        server,
        SECRET.to_string(),
        require_token,
        additional_allowed_origins,
    )
}

async fn send(router: &Router, method: Method, uri: &str, headers: &[(&str, &str)]) -> StatusCode {
    send_response(router, method, uri, headers).await.status()
}

async fn send_response(
    router: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> Response<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).unwrap();
    router.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn acp_requests_without_token_are_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    for method in [Method::GET, Method::POST, Method::DELETE] {
        let status = send(&router, method.clone(), "/acp", &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "method: {method}");
    }
}

#[tokio::test]
async fn websocket_handshake_without_token_is_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGVzdGtleTEyMzQ1Njc4OQ=="),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn websocket_handshake_rejects_arbitrary_web_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "https://evil.example"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn acp_router_websocket_handshake_rejects_arbitrary_web_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_acp_router(&dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "https://evil.example"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn acp_router_websocket_handshake_rejects_file_origins_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_acp_router(&dir);

    for origin in ["null", "file://"] {
        let status = send(
            &router,
            Method::GET,
            "/acp",
            &[
                ("origin", origin),
                ("connection", "upgrade"),
                ("upgrade", "websocket"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn authenticated_acp_router_allows_packaged_desktop_null_websocket_origin() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_authenticated_acp_router(&dir);

    let status = send(
        &router,
        Method::GET,
        &format!("/acp?token={SECRET}"),
        &[
            ("origin", "null"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn authenticated_acp_router_allows_packaged_desktop_file_websocket_origin() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_authenticated_acp_router(&dir);

    let status = send(
        &router,
        Method::GET,
        &format!("/acp?token={SECRET}"),
        &[
            ("origin", "file://"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn authenticated_serve_router_allows_null_websocket_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let status = send(
        &router,
        Method::GET,
        &format!("/acp?token={SECRET}"),
        &[
            ("origin", "null"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn authenticated_serve_router_allows_file_websocket_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let status = send(
        &router,
        Method::GET,
        &format!("/acp?token={SECRET}"),
        &[
            ("origin", "file://"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn unauthenticated_serve_router_rejects_null_websocket_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "null"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unauthenticated_serve_router_rejects_file_websocket_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "file://"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn websocket_handshake_allows_loopback_web_origins_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "http://localhost:5173"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn websocket_handshake_allows_ipv6_loopback_web_origins_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "http://[::1]:5173"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn websocket_handshake_explicit_origins_replace_loopback_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "http://localhost:5173"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn websocket_handshake_allows_configured_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    let status = send(
        &router,
        Method::GET,
        "/acp",
        &[
            ("origin", "app://localhost"),
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn websocket_handshake_explicit_origins_replace_file_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    for origin in ["null", "file://"] {
        let status = send(
            &router,
            Method::GET,
            "/acp",
            &[
                ("origin", origin),
                ("connection", "upgrade"),
                ("upgrade", "websocket"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn header_token_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    // 406 (missing Accept: text/event-stream) proves the request passed auth.
    let status = send(&router, Method::GET, "/acp", &[("X-Secret-Key", SECRET)]).await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn query_token_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let uri = format!("/acp?token={SECRET}");
    let status = send(&router, Method::GET, &uri, &[]).await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn wrong_token_is_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let status = send(&router, Method::GET, "/acp", &[("X-Secret-Key", "nope")]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let status = send(&router, Method::GET, "/acp?token=nope", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_endpoints_skip_token_check() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    for path in ["/health", "/status"] {
        let status = send(&router, Method::GET, path, &[]).await;
        assert_eq!(status, StatusCode::OK, "path: {path}");
    }
}

#[tokio::test]
async fn acp_open_when_auth_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let status = send(&router, Method::GET, "/acp", &[]).await;
    assert_eq!(status, StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn acp_cors_rejects_arbitrary_web_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "https://evil.example"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn acp_cors_rejects_custom_app_origins_unless_configured() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "app://localhost"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn acp_cors_allows_loopback_web_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "http://localhost:5173"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
}

#[tokio::test]
async fn acp_cors_allows_ipv6_loopback_web_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "http://[::1]:5173"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://[::1]:5173")
    );
}

#[tokio::test]
async fn authenticated_acp_cors_preflight_skips_token_check() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_authenticated_acp_router(&dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "http://localhost:5173"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
}

#[tokio::test]
async fn authenticated_acp_cors_allows_packaged_desktop_null_origin() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_authenticated_acp_router(&dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("null")
    );
}

#[tokio::test]
async fn authenticated_acp_cors_allows_packaged_desktop_file_origin() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_authenticated_acp_router(&dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "file://"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("file://")
    );
}

#[tokio::test]
async fn authenticated_serve_cors_allows_null_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("null")
    );
}

#[tokio::test]
async fn authenticated_serve_cors_allows_file_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(true, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "file://"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("file://")
    );
}

#[tokio::test]
async fn unauthenticated_serve_cors_rejects_null_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "null"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn unauthenticated_serve_cors_rejects_file_origin_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router(false, &dir);

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "file://"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,x-secret-key,acp-connection-id",
            ),
        ],
    )
    .await;

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn acp_cors_explicit_origins_replace_loopback_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "http://localhost:5173"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[tokio::test]
async fn acp_cors_allows_additional_configured_origins() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    let response = send_response(
        &router,
        Method::OPTIONS,
        "/acp",
        &[
            ("Origin", "app://localhost"),
            ("Access-Control-Request-Method", "POST"),
            (
                "Access-Control-Request-Headers",
                "content-type,acp-connection-id",
            ),
        ],
    )
    .await;

    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("app://localhost")
    );
}

#[tokio::test]
async fn acp_cors_explicit_origins_replace_file_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let router = test_router_with_origins(
        false,
        &dir,
        vec![HeaderValue::from_static("app://localhost")],
    );

    for origin in ["null", "file://"] {
        let response = send_response(
            &router,
            Method::OPTIONS,
            "/acp",
            &[
                ("Origin", origin),
                ("Access-Control-Request-Method", "POST"),
                (
                    "Access-Control-Request-Headers",
                    "content-type,x-secret-key,acp-connection-id",
                ),
            ],
        )
        .await;

        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }
}
