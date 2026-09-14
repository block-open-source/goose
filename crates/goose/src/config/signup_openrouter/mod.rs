pub mod server;

#[cfg(test)]
mod tests;

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{distr::Alphanumeric, RngExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// Default models for openrouter config configuration
const OPENROUTER_DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

const OPENROUTER_AUTH_URL: &str = "https://openrouter.ai/auth";
const OPENROUTER_TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const CALLBACK_URL: &str = "http://localhost:3000";
const AUTH_TIMEOUT: Duration = Duration::from_secs(180); // 3 minutes

#[derive(Debug)]
pub struct PkceAuthFlow {
    code_verifier: String,
    code_challenge: String,
    server_shutdown_tx: Option<oneshot::Sender<()>>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    key: String,
}

#[derive(Debug, Serialize)]
struct TokenRequest {
    code: String,
    code_verifier: String,
    code_challenge_method: String,
}

impl PkceAuthFlow {
    pub fn new() -> Result<Self> {
        let code_verifier: String = rand::rng()
            .sample_iter(&Alphanumeric)
            .take(128)
            .map(char::from)
            .collect();

        let mut hasher = Sha256::new();
        hasher.update(&code_verifier);
        let hash = hasher.finalize();

        let code_challenge = URL_SAFE_NO_PAD.encode(hash);

        Ok(Self {
            code_verifier,
            code_challenge,
            server_shutdown_tx: None,
        })
    }

    pub fn get_auth_url(&self) -> String {
        format!(
            "{}?callback_url={}&code_challenge={}&code_challenge_method=S256",
            OPENROUTER_AUTH_URL,
            urlencoding::encode(CALLBACK_URL),
            urlencoding::encode(&self.code_challenge)
        )
    }

    /// Start local server and wait for callback
    pub async fn start_server(&mut self) -> Result<String> {
        let (code_tx, code_rx) = oneshot::channel::<String>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Store shutdown sender so we can stop the server later
        self.server_shutdown_tx = Some(shutdown_tx);

        // Start the server in a background task
        tokio::spawn(async move {
            if let Err(e) = server::run_callback_server(code_tx, shutdown_rx).await {
                eprintln!("Server error: {}", e);
            }
        });

        // Wait for the authorization code with timeout
        match timeout(AUTH_TIMEOUT, code_rx).await {
            Ok(Ok(code)) => Ok(code),
            Ok(Err(_)) => Err(anyhow!("Failed to receive authorization code")),
            Err(_) => Err(anyhow!("Authentication timeout - please try again")),
        }
    }

    pub async fn exchange_code(&self, code: String) -> Result<String> {
        let client = Client::new();

        let request_body = TokenRequest {
            code: code.clone(),
            code_verifier: self.code_verifier.clone(),
            code_challenge_method: "S256".to_string(),
        };

        eprintln!("Exchanging code for API key...");
        eprintln!("Code: {}", code);
        eprintln!("Code verifier length: {}", self.code_verifier.len());
        eprintln!("Code challenge: {}", self.code_challenge);

        let response = client
            .post(OPENROUTER_TOKEN_URL)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            eprintln!("Token exchange failed!");
            eprintln!("Status: {}", status);
            eprintln!("Error response: {}", error_text);
            return Err(anyhow!(
                "Failed to exchange code: {} - {}",
                status,
                error_text
            ));
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(token_response.key)
    }

    /// Complete flow: open browser, wait for callback, exchange code
    pub async fn complete_flow(&mut self) -> Result<String> {
        let auth_url = self.get_auth_url();

        println!("Opening browser for authentication...");
        eprintln!("Auth URL: {}", auth_url);

        if let Err(e) = webbrowser::open(&auth_url) {
            eprintln!("Failed to open browser automatically: {}", e);
            println!("Please open this URL manually: {}", auth_url);
        }

        println!("Waiting for authentication callback...");
        let code = self.start_server().await?;

        println!("Authorization code received. Exchanging for API key...");
        eprintln!("Received code: {}", code);

        let api_key = self.exchange_code(code).await?;

        // Shutdown the server if it's still running
        if let Some(tx) = self.server_shutdown_tx.take() {
            let _ = tx.send(());
        }

        Ok(api_key)
    }
}

pub use self::PkceAuthFlow as OpenRouterAuth;

use crate::config::Config;

pub fn configure_openrouter(config: &Config, api_key: String) -> Result<()> {
    config.set_secret("OPENROUTER_API_KEY", &api_key)?;
    crate::config::set_active_provider(
        config,
        goose_providers::openrouter::OPENROUTER_PROVIDER_NAME,
        OPENROUTER_DEFAULT_MODEL,
    )?;
    Ok(())
}
