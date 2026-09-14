//! Format-agnostic HTTP status → `ProviderError` mapping.
//!
//! Used by providers regardless of their wire format (OpenAI, Anthropic,
//! Google, etc.). Parses both `{"error":{"message":"..."}}` and
//! `{"message":"..."}` error shapes.

use std::time::{Duration, SystemTime};

use crate::errors::ProviderError;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use futures::TryStreamExt;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub const MAX_PROVIDER_JSON_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Strip credentials and sensitive query parameters from a URL for safe
/// inclusion in error messages and logs. Drops userinfo (`user:pass@`) and
/// all query parameters (which may contain API keys like `?key=...`).
/// Returns the original string unchanged if it doesn't parse as a URL
/// (e.g. a bare path like "v1/models").
pub fn sanitize_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    if !url.username().is_empty() || url.password().is_some() {
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }
    url.set_query(None);
    url.to_string()
}

/// Hard cap on retry delays we'll honor from remote responses. A malformed
/// 429 with `retry_after_seconds: 1e30` (or a far-future HTTP-date) should
/// degrade to "no retry hint" rather than freeze the agent or panic when
/// converting to `Duration`. One hour is well past any legitimate
/// rate-limit window.
const MAX_RETRY_AFTER_SECS: f64 = 3600.0;

/// Extract a retry delay from a 429 response. Prefers the body's
/// `error.metadata.retry_after_seconds` (OpenRouter shape, more precise than
/// the integer header) and falls back to the RFC 7231 `Retry-After` header
/// in either its delay-seconds form or its HTTP-date form.
fn extract_retry_after(headers: &HeaderMap, payload: Option<&Value>) -> Option<Duration> {
    if let Some(secs) = payload
        .and_then(|p| p.get("error"))
        .and_then(|e| e.get("metadata"))
        .and_then(|m| m.get("retry_after_seconds"))
        .and_then(|v| v.as_f64())
    {
        if let Some(d) = duration_from_finite_secs(secs) {
            return Some(d);
        }
    }

    headers
        .get(RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| parse_retry_after_header(s.trim()))
}

/// Convert a finite, non-negative, in-range seconds value to a `Duration`.
/// Returns `None` for NaN, negative, infinite, or absurdly large inputs —
/// `Duration::from_secs_f64` panics on the latter.
fn duration_from_finite_secs(secs: f64) -> Option<Duration> {
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    let clamped = secs.min(MAX_RETRY_AFTER_SECS);
    Some(Duration::from_secs_f64(clamped))
}

/// Parse `Retry-After` per RFC 7231 §7.1.3: either a non-negative integer
/// number of seconds, or an HTTP-date (interpreted as the absolute time at
/// which the request may be retried). A past date is honored as "retry
/// now" (`Duration::ZERO`) rather than dropped — clock skew or near-now
/// timestamps plus network latency commonly produce an HTTP-date that is
/// already in the past, and falling back to exponential backoff would
/// add unnecessary delay against an explicit server hint.
fn parse_retry_after_header(value: &str) -> Option<Duration> {
    if let Ok(secs) = value.parse::<u64>() {
        return duration_from_finite_secs(secs as f64);
    }
    let target = parse_http_date(value)?;
    let delay = target
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    duration_from_finite_secs(delay.as_secs_f64())
}

/// Parse the three HTTP-date forms RFC 7231 §7.1.1.1 requires recipients to
/// accept: IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`), the obsolete RFC 850
/// form (`Sunday, 06-Nov-94 08:49:37 GMT`), and asctime (`Sun Nov  6 08:49:37
/// 1994`). All three are interpreted as GMT.
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let value = value.trim();
    if let Ok(dt) = DateTime::parse_from_rfc2822(value) {
        return Some(SystemTime::from(dt));
    }
    if let Some(body) = value.strip_suffix(" GMT") {
        if let Ok(naive) = NaiveDateTime::parse_from_str(body, "%A, %d-%b-%y %H:%M:%S") {
            return Some(SystemTime::from(Utc.from_utc_datetime(&naive)));
        }
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y") {
        return Some(SystemTime::from(Utc.from_utc_datetime(&naive)));
    }
    None
}

fn is_context_length_exceeded(payload: Option<&Value>, message: &str) -> bool {
    let payload_exceeded = payload
        .and_then(|payload| payload.get("error"))
        .is_some_and(|error| {
            error
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| code.eq_ignore_ascii_case("context_length_exceeded"))
                || match (
                    error.get("n_prompt_tokens").and_then(Value::as_f64),
                    error.get("n_ctx").and_then(Value::as_f64),
                ) {
                    (Some(prompt_tokens), Some(context_limit)) => {
                        context_limit > 0.0 && prompt_tokens > context_limit
                    }
                    _ => false,
                }
        });

    payload_exceeded || is_context_length_exceeded_message(message)
}

fn is_context_length_exceeded_message(text: &str) -> bool {
    let text_lower = text.to_lowercase();

    let direct_context_phrases = [
        "context length",
        "context_length_exceeded",
        "context window",
        "context_window_exceeded",
        "context limit",
        "maximum context",
        "max context",
        "maximum prompt length",
        "max prompt length",
    ];
    if direct_context_phrases
        .iter()
        .any(|phrase| text_lower.contains(phrase))
    {
        return true;
    }

    if text_lower.contains("reduce the length")
        && ["message", "messages", "input", "prompt"]
            .iter()
            .any(|word| text_lower.contains(word))
    {
        return true;
    }

    if [
        "input is too long",
        "input too long",
        "prompt is too long",
        "prompt too long",
    ]
    .iter()
    .any(|phrase| text_lower.contains(phrase))
    {
        return true;
    }

    let mentions_prompt_input_tokens = [
        "input token",
        "input length",
        "prompt token",
        "prompt length",
        "message token",
        "messages token",
        "request token",
        "total token",
    ]
    .iter()
    .any(|phrase| text_lower.contains(phrase));
    let mentions_limit = [
        "model limit",
        "model's limit",
        "maximum allowed",
        "max allowed",
        "maximum number of tokens",
        "token limit",
        "tokens limit",
    ]
    .iter()
    .any(|phrase| text_lower.contains(phrase));
    let mentions_overflow = ["exceed", "too long", "too large", "over the limit"]
        .iter()
        .any(|phrase| text_lower.contains(phrase));

    let words = text_lower.split(|character: char| !character.is_ascii_alphanumeric());
    let mentions_request = words.clone().any(|word| word == "request");
    let mentions_bytes = words.clone().any(|word| matches!(word, "byte" | "bytes"));
    let mentions_content_length = ["content length", "content-length"]
        .iter()
        .any(|phrase| text_lower.contains(phrase));
    let mentions_request_data_size = [
        "request size",
        "requestsize",
        "request body size",
        "request payload size",
        "payload size",
        "body size",
    ]
    .iter()
    .any(|phrase| text_lower.contains(phrase));
    let request_data_too_large = [
        "request body is too large",
        "request body too large",
        "request payload is too large",
        "request payload too large",
        "payload is too large",
        "payload too large",
    ]
    .iter()
    .any(|phrase| text_lower.contains(phrase));
    let mentions_byte_limit = mentions_request_data_size
        || request_data_too_large
        || (mentions_content_length && (mentions_request || mentions_bytes));
    if mentions_byte_limit && mentions_overflow {
        return true;
    }

    mentions_prompt_input_tokens && mentions_limit && mentions_overflow
}

pub fn map_http_error_to_provider_error(
    status: StatusCode,
    payload: Option<Value>,
    url: &str,
) -> ProviderError {
    let extract_message = || -> String {
        payload
            .as_ref()
            .and_then(|p| {
                p.get("error")
                    .and_then(|e| e.get("message"))
                    .or_else(|| p.get("message"))
                    .and_then(|m| m.as_str())
                    .map(String::from)
            })
            .unwrap_or_else(|| payload.as_ref().map(|p| p.to_string()).unwrap_or_default())
    };

    let error = match status {
        StatusCode::OK => unreachable!("Should not call this function with OK status"),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Authentication(format!(
            "Authentication failed for {url}. Status: {}. Response: {}",
            status,
            extract_message()
        )),
        StatusCode::NOT_FOUND => ProviderError::RequestFailed(format!(
            "Resource not found (404) at {url}: {}",
            extract_message()
        )),
        StatusCode::PAYMENT_REQUIRED => ProviderError::CreditsExhausted {
            details: extract_message(),
            top_up_url: None,
        },
        StatusCode::PAYLOAD_TOO_LARGE => ProviderError::ContextLengthExceeded(extract_message()),
        StatusCode::BAD_REQUEST => {
            let payload_str = extract_message();
            if is_context_length_exceeded(payload.as_ref(), &payload_str) {
                ProviderError::ContextLengthExceeded(payload_str)
            } else {
                ProviderError::RequestFailed(format!("Bad request (400): {}", payload_str))
            }
        }
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimitExceeded {
            details: extract_message(),
            retry_delay: None,
        },
        _ if status.is_server_error() => ProviderError::ServerError(format!(
            "Server error ({}) at {url}: {}",
            status,
            extract_message()
        )),
        _ => ProviderError::RequestFailed(format!(
            "Request failed with status {} at {url}: {}",
            status,
            extract_message()
        )),
    };

    if !status.is_success() {
        tracing::warn!(
            "Provider request failed with status: {}. Payload: {:?}. Returning error: {:?}",
            status,
            payload,
            error
        );
    }

    error
}

#[derive(Clone, Copy)]
pub struct ResponseDeadline(tokio::time::Instant);

pub fn set_response_deadline(response: &mut Response, deadline: tokio::time::Instant) {
    response.extensions_mut().insert(ResponseDeadline(deadline));
}

pub async fn send_bounded(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<Response, ProviderError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut response = tokio::time::timeout_at(deadline, request.send())
        .await
        .map_err(|_| {
            ProviderError::NetworkError(
                "Request timed out — check your network connection and try again.".to_string(),
            )
        })??;
    set_response_deadline(&mut response, deadline);
    Ok(response)
}

async fn read_response_body_with_limit(
    response: Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let deadline = response.extensions().get::<ResponseDeadline>().copied();
    let read = async move {
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();

        while let Some(chunk) = stream.try_next().await.map_err(|e| {
            ProviderError::RequestFailed(format!("Failed to read response body: {e}"))
        })? {
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(ProviderError::RequestFailed(format!(
                    "Provider response body exceeds the {limit} byte limit"
                )));
            }
            body.try_reserve(chunk.len()).map_err(|_| {
                ProviderError::RequestFailed("Failed to allocate response body".to_string())
            })?;
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    };

    match deadline {
        Some(ResponseDeadline(deadline)) => {
            tokio::time::timeout_at(deadline, read).await.map_err(|_| {
                ProviderError::NetworkError(
                    "Response body timed out — check your network connection and try again."
                        .to_string(),
                )
            })?
        }
        None => read.await,
    }
}

pub async fn read_error_body(response: Response) -> Option<String> {
    read_response_body_with_limit(response, MAX_PROVIDER_JSON_RESPONSE_BYTES)
        .await
        .ok()
        .map(|body| String::from_utf8_lossy(&body).into_owned())
}

pub async fn read_json_response<T: DeserializeOwned>(
    response: Response,
) -> Result<T, ProviderError> {
    read_json_response_with_limit(response, MAX_PROVIDER_JSON_RESPONSE_BYTES).await
}

async fn read_json_response_with_limit<T: DeserializeOwned>(
    response: Response,
    limit: usize,
) -> Result<T, ProviderError> {
    let body = read_response_body_with_limit(response, limit).await?;
    serde_json::from_slice(&body)
        .map_err(|e| ProviderError::RequestFailed(format!("Response body is not valid JSON: {e}")))
}

pub async fn handle_status(response: Response) -> Result<Response, ProviderError> {
    handle_status_with_limit(response, MAX_PROVIDER_JSON_RESPONSE_BYTES).await
}

async fn handle_status_with_limit(
    response: Response,
    limit: usize,
) -> Result<Response, ProviderError> {
    let status = response.status();
    if !status.is_success() {
        let url = sanitize_url(response.url().as_str());
        let headers = response.headers().clone();
        let body = read_response_body_with_limit(response, limit)
            .await
            .unwrap_or_default();
        let body = String::from_utf8_lossy(&body);
        let payload = serde_json::from_str::<Value>(&body).ok();
        let mut err = map_http_error_to_provider_error(status, payload.clone(), &url);
        if let ProviderError::RateLimitExceeded { details, .. } = &err {
            err = ProviderError::RateLimitExceeded {
                details: details.clone(),
                retry_delay: extract_retry_after(&headers, payload.as_ref()),
            };
        }
        return Err(err);
    }
    Ok(response)
}

pub async fn handle_response(response: Response) -> Result<Value, ProviderError> {
    handle_response_with_limit(response, MAX_PROVIDER_JSON_RESPONSE_BYTES).await
}

async fn handle_response_with_limit(
    response: Response,
    limit: usize,
) -> Result<Value, ProviderError> {
    let response = handle_status_with_limit(response, limit).await?;
    read_json_response_with_limit(response, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use serde_json::json;
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    fn headers_with_retry_after(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, value.parse().unwrap());
        h
    }

    async fn response_from_raw(raw_response: Vec<u8>) -> Response {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            socket.write_all(&raw_response).await.unwrap();
        });

        reqwest::Client::new()
            .get(format!("http://{addr}"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bounded_json_accepts_body_without_content_length() {
        let response = response_from_raw(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"ok\":true}"
                .to_vec(),
        )
        .await;

        let value: Value = read_json_response_with_limit(response, 64).await.unwrap();
        assert_eq!(value, json!({"ok": true}));
    }

    #[tokio::test]
    async fn bounded_json_rejects_oversized_chunked_body() {
        let body = format!("{{\"value\":\"{}\"}}", "a".repeat(64));
        let raw = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        )
        .into_bytes();
        let response = response_from_raw(raw).await;

        let err = read_json_response_with_limit::<Value>(response, 64)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("64 byte limit"), "got: {err}");
    }

    #[tokio::test]
    async fn bounded_handle_response_rejects_oversized_success_body() {
        let body = format!("{{\"value\":\"{}\"}}", "a".repeat(64));
        let raw = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        )
        .into_bytes();
        let response = response_from_raw(raw).await;

        let err = handle_response_with_limit(response, 64).await.unwrap_err();
        assert!(err.to_string().contains("64 byte limit"), "got: {err}");
    }

    #[tokio::test]
    async fn bounded_status_preserves_authentication_for_oversized_error_body() {
        let body = format!("{{\"error\":{{\"message\":\"{}\"}}}}", "a".repeat(64));
        let raw = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        )
        .into_bytes();
        let response = response_from_raw(raw).await;

        let err = handle_status_with_limit(response, 64).await.unwrap_err();
        assert!(
            matches!(err, ProviderError::Authentication(_)),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn bounded_status_preserves_retry_after_for_oversized_error_body() {
        let body = format!("{{\"error\":{{\"message\":\"{}\"}}}}", "a".repeat(64));
        let raw = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 17\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        )
        .into_bytes();
        let response = response_from_raw(raw).await;

        let err = handle_status_with_limit(response, 64).await.unwrap_err();
        assert!(
            matches!(
                err,
                ProviderError::RateLimitExceeded {
                    retry_delay: Some(delay),
                    ..
                } if delay == Duration::from_secs(17)
            ),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn bounded_json_accepts_many_small_chunks() {
        let body = br#"{"ok":true}"#;
        let mut raw =
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
                .to_vec();
        for byte in body {
            raw.extend_from_slice(b"1\r\n");
            raw.push(*byte);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(b"0\r\n\r\n");

        let response = response_from_raw(raw).await;
        let value: Value = read_json_response_with_limit(response, 64).await.unwrap();
        assert_eq!(value, json!({"ok": true}));
    }

    #[tokio::test]
    async fn bounded_json_limits_decompressed_body() {
        let server = MockServer::start().await;
        let body = format!("{{\"value\":\"{}\"}}", "a".repeat(128));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .insert_header("content-encoding", "gzip")
                    .set_body_bytes(compressed),
            )
            .mount(&server)
            .await;

        let response = reqwest::Client::new()
            .get(server.uri())
            .send()
            .await
            .unwrap();
        let err = read_json_response_with_limit::<Value>(response, 64)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("64 byte limit"), "got: {err}");
    }

    fn error_payload<const N: usize>(fields: [(&str, Value); N]) -> Value {
        let mut error = json!({ "message": "invalid request" });
        let error = error.as_object_mut().unwrap();
        for (key, value) in fields {
            error.insert(key.to_string(), value);
        }
        json!({ "error": error })
    }

    #[test]
    fn retry_after_prefers_body_seconds_over_header() {
        let payload = json!({
            "error": {
                "metadata": { "retry_after_seconds": 22.148 }
            }
        });
        let headers = headers_with_retry_after("5");
        let delay = extract_retry_after(&headers, Some(&payload));
        assert_eq!(delay, Some(Duration::from_secs_f64(22.148)));
    }

    #[test]
    fn retry_after_falls_back_to_header_when_body_missing() {
        let headers = headers_with_retry_after("17");
        let delay = extract_retry_after(&headers, None);
        assert_eq!(delay, Some(Duration::from_secs(17)));
    }

    #[test]
    fn retry_after_returns_none_when_neither_present() {
        let payload = json!({ "error": { "message": "rate limited" } });
        let delay = extract_retry_after(&empty_headers(), Some(&payload));
        assert!(delay.is_none());
    }

    #[test]
    fn retry_after_ignores_negative_or_nan_body_seconds() {
        let payload = json!({ "error": { "metadata": { "retry_after_seconds": -1.0 } } });
        assert!(extract_retry_after(&empty_headers(), Some(&payload)).is_none());

        let payload = json!({ "error": { "metadata": { "retry_after_seconds": "not a number" } } });
        assert!(extract_retry_after(&empty_headers(), Some(&payload)).is_none());
    }

    #[test]
    fn retry_after_past_http_date_means_retry_now() {
        // RFC 7231 allows an HTTP-date in `Retry-After`; a past date means
        // "you may retry now" — surface that as `Duration::ZERO` rather
        // than `None`, so we honor the server hint instead of dropping
        // back to exponential backoff (clock skew or near-now timestamps
        // plus latency commonly land us here).
        let headers = headers_with_retry_after("Fri, 31 Dec 1999 23:59:59 GMT");
        let delay = extract_retry_after(&headers, None);
        assert_eq!(delay, Some(Duration::ZERO));
    }

    #[test]
    fn retry_after_future_http_date_parsed() {
        // A future HTTP-date should produce a positive duration up to the cap.
        let target = chrono::Utc::now() + chrono::Duration::seconds(45);
        let header_value = target.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let headers = headers_with_retry_after(&header_value);
        let delay = extract_retry_after(&headers, None).expect("should parse future HTTP-date");
        // Allow some slack for the clock advancing between header creation and parse.
        assert!(
            delay >= Duration::from_secs(30) && delay <= Duration::from_secs(60),
            "expected ~45s, got {delay:?}"
        );
    }

    #[test]
    fn retry_after_parses_rfc850_http_date() {
        // RFC 7231 recipients must accept the obsolete RFC 850 date syntax.
        // Build a future date in that form and assert it round-trips.
        let target = chrono::Utc::now() + chrono::Duration::seconds(90);
        let header_value = target.format("%A, %d-%b-%y %H:%M:%S GMT").to_string();
        let headers = headers_with_retry_after(&header_value);
        let delay = extract_retry_after(&headers, None).expect("rfc850 date should parse");
        assert!(
            delay >= Duration::from_secs(60) && delay <= Duration::from_secs(120),
            "expected ~90s, got {delay:?}"
        );
    }

    #[test]
    fn retry_after_parses_asctime_http_date() {
        // The third HTTP-date form RFC 7231 requires recipients to accept.
        // asctime has no timezone marker; we interpret it as GMT.
        let target = chrono::Utc::now() + chrono::Duration::seconds(120);
        let header_value = target.format("%a %b %e %H:%M:%S %Y").to_string();
        let headers = headers_with_retry_after(&header_value);
        let delay = extract_retry_after(&headers, None).expect("asctime date should parse");
        assert!(
            delay >= Duration::from_secs(90) && delay <= Duration::from_secs(150),
            "expected ~120s, got {delay:?}"
        );
    }

    #[test]
    fn retry_after_clamps_absurd_body_seconds() {
        // `Duration::from_secs_f64(1e30)` panics; the clamp keeps the agent alive.
        let payload = json!({ "error": { "metadata": { "retry_after_seconds": 1e30 } } });
        let delay = extract_retry_after(&empty_headers(), Some(&payload));
        assert_eq!(delay, Some(Duration::from_secs_f64(MAX_RETRY_AFTER_SECS)));
    }

    #[test]
    fn retry_after_clamps_infinite_body_seconds() {
        let payload = json!({ "error": { "metadata": { "retry_after_seconds": f64::INFINITY } } });
        assert!(extract_retry_after(&empty_headers(), Some(&payload)).is_none());
    }

    #[test]
    fn context_length_classifier_accepts_context_window_errors() {
        let messages = [
            "This request exceeds the maximum context length",
            "context_length_exceeded",
            "context window exceeded",
            "Input token count exceeds the maximum number of tokens allowed",
            "Please reduce the length of the messages",
            "prompt is too long for this model",
            "Server received a request which exceeds maximum allowed content length. RequestSize(bytes): 34021227, Limit(bytes): 33554432.",
            "Request body size exceeds the maximum allowed limit",
            "Request body is too large",
            "Request payload too large",
            "Content-Length exceeds the maximum allowed request size",
        ];

        for message in messages {
            assert!(
                is_context_length_exceeded_message(message),
                "expected context-length match for: {message}"
            );
        }
    }

    #[test]
    fn context_length_classifier_rejects_generic_bad_request_errors() {
        let messages = [
            "max_tokens must be less than or equal to 4096",
            "Requested max_tokens exceeds the model output limit",
            "Current token count exceeds your organization quota",
            "temperature exceeds maximum allowed value",
            "schema is too long",
            "metadata length exceeds maximum allowed",
            "Invalid request body: temperature exceeds maximum allowed value",
            "tools[0].description content length exceeds maximum allowed",
            "response content length exceeds maximum allowed",
        ];

        for message in messages {
            assert!(
                !is_context_length_exceeded_message(message),
                "expected generic bad request for: {message}"
            );
        }
    }

    #[test]
    fn structured_context_length_classifier_handles_supported_and_invalid_payloads() {
        let cases = [
            (
                "context overflow code",
                error_payload([("code", json!("Context_Length_Exceeded"))]),
                true,
            ),
            (
                "prompt exceeds context limit",
                error_payload([
                    ("code", json!(400)),
                    ("n_prompt_tokens", json!(49202)),
                    ("n_ctx", json!(49152)),
                ]),
                true,
            ),
            (
                "generic output token limit",
                error_payload([(
                    "message",
                    json!("max_tokens must be less than or equal to 4096"),
                )]),
                false,
            ),
            (
                "prompt count only",
                error_payload([("n_prompt_tokens", json!(49202))]),
                false,
            ),
            (
                "null token counts",
                error_payload([("n_prompt_tokens", Value::Null), ("n_ctx", Value::Null)]),
                false,
            ),
            (
                "zero context limit",
                error_payload([("n_prompt_tokens", json!(1)), ("n_ctx", json!(0))]),
                false,
            ),
            (
                "equal token counts",
                error_payload([("n_prompt_tokens", json!(49152)), ("n_ctx", json!(49152))]),
                false,
            ),
            (
                "top-level token counts",
                json!({
                    "error": { "message": "invalid request" },
                    "n_prompt_tokens": 49202,
                    "n_ctx": 49152
                }),
                false,
            ),
        ];

        for (case, payload, expected_context_overflow) in cases {
            let error = map_http_error_to_provider_error(
                StatusCode::BAD_REQUEST,
                Some(payload),
                "http://test/endpoint",
            );
            assert_eq!(
                matches!(&error, ProviderError::ContextLengthExceeded(_)),
                expected_context_overflow,
                "unexpected classification for {case}: {error:?}"
            );
        }
    }
}
