//! End-to-end coverage for command-based auth on custom providers (issue #11329):
//! a `DeclarativeProviderConfig.auth` command is run to fetch the credential
//! instead of a static `api_key_env`, and the credential is refreshed
//! reactively when the upstream API returns a 401.

use goose::conversation::message::Message;
use goose::providers::base::Provider;
use goose::providers::openai_def;
use goose_providers::declarative::{AuthConfig, DeclarativeProviderConfig, ProviderEngine};
use goose_providers::model::ModelConfig;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn custom_config_with_auth(base_url: &str, auth: AuthConfig) -> DeclarativeProviderConfig {
    DeclarativeProviderConfig {
        name: "custom_command_auth".to_string(),
        engine: ProviderEngine::OpenAI,
        display_name: "Custom Command Auth".to_string(),
        description: None,
        api_key_env: String::new(),
        base_url: base_url.to_string(),
        models: vec![goose_providers::base::ModelInfo::new("test-model")],
        headers: None,
        session_id_header_override: None,
        timeout_seconds: None,
        supports_streaming: Some(true),
        requires_auth: true,
        catalog_provider_id: None,
        base_path: None,
        env_vars: None,
        auth: Some(auth),
        dynamic_models: Some(false),
        skip_canonical_filtering: false,
        model_doc_link: None,
        setup_steps: vec![],
        toolshim: false,
        preserves_thinking: false,
        emit_clear_thinking: false,
        setup: None,
    }
}

/// A command whose stdout differs on every invocation (this process's own
/// PID / a fresh pseudo-random draw), so two captured values back-to-back
/// prove whether the underlying command was actually re-run.
#[cfg(unix)]
fn distinct_value_command() -> (&'static str, Vec<&'static str>) {
    ("sh", vec!["-c", "echo $$"])
}
#[cfg(windows)]
fn distinct_value_command() -> (&'static str, Vec<&'static str>) {
    ("cmd", vec!["/C", "echo %RANDOM%%RANDOM%%RANDOM%"])
}

fn chat_completions_sse() -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({
            "choices": [{
                "delta": {"content": "hi", "role": "assistant"},
                "index": 0
            }],
            "created": 1755133833,
            "id": "chatcmpl-test",
            "model": "test-model"
        }),
        json!({
            "choices": [],
            "usage": {"completion_tokens": 1, "prompt_tokens": 1, "total_tokens": 2}
        })
    )
}

async fn complete(provider: &dyn Provider) -> anyhow::Result<()> {
    let message = Message::user().with_text("hello");
    let model_config = ModelConfig::new("test-model");
    provider
        .complete(&model_config, "system", &[message], &[])
        .await?;
    Ok(())
}

#[tokio::test]
async fn command_auth_credential_is_sent_as_bearer_token() {
    let server = MockServer::start().await;
    let captured_auth: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = captured_auth.clone();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &Request| {
            let auth = req
                .headers
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            capture.lock().unwrap().push(auth);
            ResponseTemplate::new(200)
                .set_body_string(chat_completions_sse())
                .insert_header("content-type", "text/event-stream")
        })
        .mount(&server)
        .await;

    let config = custom_config_with_auth(
        &server.uri(),
        AuthConfig {
            command: "echo".to_string(),
            args: vec!["test-token-123".to_string()],
            refresh_interval: 3600,
            timeout_seconds: None,
            cwd: None,
        },
    );
    let provider = openai_def::from_custom_config(config, None).unwrap();

    complete(&provider).await.unwrap();

    assert_eq!(
        captured_auth.lock().unwrap().as_slice(),
        ["Bearer test-token-123".to_string()]
    );
}

#[tokio::test]
async fn command_auth_refreshes_credential_on_401_and_retries() {
    let server = MockServer::start().await;
    let captured_auth: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = captured_auth.clone();
    let call_count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &Request| {
            let auth = req
                .headers
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            capture.lock().unwrap().push(auth);

            if call_count.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error": {"message": "token expired"}}))
            } else {
                ResponseTemplate::new(200)
                    .set_body_string(chat_completions_sse())
                    .insert_header("content-type", "text/event-stream")
            }
        })
        .mount(&server)
        .await;

    // A value that differs on every invocation (this process's own PID / a
    // fresh pseudo-random draw), so two distinct captured Authorization
    // headers prove the auth command actually ran twice (once up front, once
    // after `refresh_credentials` invalidated the cache on the 401), rather
    // than the same token being reused.
    let (command, args) = distinct_value_command();
    let config = custom_config_with_auth(
        &server.uri(),
        AuthConfig {
            command: command.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            refresh_interval: 3600,
            timeout_seconds: None,
            cwd: None,
        },
    );
    let provider = openai_def::from_custom_config(config, None).unwrap();

    complete(&provider)
        .await
        .expect("request should succeed after refreshing credentials and retrying");

    let captured = captured_auth.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected an initial request and a retry");
    assert_ne!(
        captured[0], captured[1],
        "the retried request should carry a freshly-fetched credential, not the stale one"
    );
}
