use reqwest::header::{HeaderName, HeaderValue};

pub const SESSION_ID_HEADER: &str = "agent-session-id";

pub const TOOL_CALL_REQUEST_ID_HEADER: &str = "agent-tool-call-request-id";
pub const WORKING_DIR_HEADER: &str = "agent-working-dir";

tokio::task_local! {
    pub static SESSION_ID: Option<String>;
}

pub async fn with_session_id<F>(session_id: Option<String>, f: F) -> F::Output
where
    F: std::future::Future,
{
    SESSION_ID.scope(session_id, f).await
}

pub fn current_session_id() -> Option<String> {
    SESSION_ID.try_with(|id| id.clone()).ok().flatten()
}

pub fn session_id_request_builder() -> goose_providers::api_client::RequestBuilderDecorator {
    session_id_request_builder_with_header_name(HeaderName::from_static(SESSION_ID_HEADER))
}

pub(crate) fn session_id_request_builder_with_header_override(
    header_name_override: Option<&str>,
) -> Result<goose_providers::api_client::RequestBuilderDecorator, reqwest::header::InvalidHeaderName>
{
    let header_name = match header_name_override {
        Some(header_name) => HeaderName::from_bytes(header_name.as_bytes())?,
        None => HeaderName::from_static(SESSION_ID_HEADER),
    };

    Ok(session_id_request_builder_with_header_name(header_name))
}

fn session_id_request_builder_with_header_name(
    header_name: HeaderName,
) -> goose_providers::api_client::RequestBuilderDecorator {
    std::sync::Arc::new(move |request| {
        let (client, request) = request.build_split();
        let mut request = request?;
        let session_header = header_name.clone();
        request.headers_mut().remove(&session_header);

        if let Some(session_id) = current_session_id() {
            let value = HeaderValue::from_str(&session_id)?;
            request.headers_mut().insert(session_header, value);
        }

        Ok(reqwest::RequestBuilder::from_parts(client, request))
    })
}

/// Local OS user running goose, shared by the OTLP `user.name` resource
/// attribute and the `session.user` span attribute so the two never drift.
pub fn session_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Hostname of the machine running goose, shared by the OTLP `host.name`
/// resource attribute and the `session.host` span attribute.
pub fn session_host() -> String {
    gethostname::gethostname().to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_id_available_when_set() {
        with_session_id(Some("test-session-123".to_string()), async {
            assert_eq!(current_session_id(), Some("test-session-123".to_string()));
        })
        .await;
    }

    #[tokio::test]
    async fn test_session_id_none_when_not_set() {
        let id = current_session_id();
        assert_eq!(id, None);
    }

    #[tokio::test]
    async fn test_session_id_none_when_explicitly_none() {
        with_session_id(None, async {
            assert_eq!(current_session_id(), None);
        })
        .await;
    }

    #[tokio::test]
    async fn test_session_id_none_clears_outer_scope() {
        with_session_id(Some("outer-session".to_string()), async {
            assert_eq!(current_session_id(), Some("outer-session".to_string()));

            with_session_id(None, async {
                assert_eq!(current_session_id(), None);
            })
            .await;

            assert_eq!(current_session_id(), Some("outer-session".to_string()));
        })
        .await;
    }

    #[tokio::test]
    async fn test_session_id_scoped_correctly() {
        assert_eq!(current_session_id(), None);

        with_session_id(Some("outer-session".to_string()), async {
            assert_eq!(current_session_id(), Some("outer-session".to_string()));

            with_session_id(Some("inner-session".to_string()), async {
                assert_eq!(current_session_id(), Some("inner-session".to_string()));
            })
            .await;

            assert_eq!(current_session_id(), Some("outer-session".to_string()));
        })
        .await;

        assert_eq!(current_session_id(), None);
    }

    #[tokio::test]
    async fn test_session_id_across_await_points() {
        with_session_id(Some("persistent-session".to_string()), async {
            assert_eq!(current_session_id(), Some("persistent-session".to_string()));

            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            assert_eq!(current_session_id(), Some("persistent-session".to_string()));
        })
        .await;
    }

    #[tokio::test]
    async fn test_session_id_request_builder_uses_custom_header() {
        with_session_id(Some("test-session-123".to_string()), async {
            let decorate =
                session_id_request_builder_with_header_override(Some("x-opencode-session"))
                    .unwrap();

            let request = decorate(reqwest::Client::new().get("http://localhost"))
                .unwrap()
                .build()
                .unwrap();

            assert_eq!(
                request.headers().get("x-opencode-session").unwrap(),
                "test-session-123"
            );
            assert!(request.headers().get(SESSION_ID_HEADER).is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn test_session_id_request_builder_uses_default_without_override() {
        with_session_id(Some("test-session-123".to_string()), async {
            let decorate = session_id_request_builder_with_header_override(None).unwrap();

            let request = decorate(reqwest::Client::new().get("http://localhost"))
                .unwrap()
                .build()
                .unwrap();

            assert_eq!(
                request.headers().get(SESSION_ID_HEADER).unwrap(),
                "test-session-123"
            );
        })
        .await;
    }

    #[test]
    fn test_session_id_request_builder_rejects_invalid_header_override() {
        assert!(session_id_request_builder_with_header_override(Some("invalid header")).is_err());
    }
}
