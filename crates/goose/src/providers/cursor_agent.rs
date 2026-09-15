use anyhow::Result;
use async_trait::async_trait;
use rmcp::model::Role;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::base::{
    stream_from_single_message, ConfigKey, MessageStream, Provider, ProviderDef, ProviderMetadata,
};
use super::catalog::ProviderSetupMetadata;
use super::utils::filter_extensions_from_system_prompt;
use crate::config::search_path::SearchPaths;
use crate::conversation::message::{Message, MessageContent};
use crate::subprocess::configure_subprocess;
use futures::future::BoxFuture;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use goose_providers::request_log::{start_log, LoggerHandleExt};
use rmcp::model::Tool;

const CURSOR_AGENT_PROVIDER_NAME: &str = "cursor-agent";
pub const CURSOR_AGENT_DEFAULT_MODEL: &str = "auto";
// Fallback when `cursor-agent models` cannot be queried.
pub const CURSOR_AGENT_KNOWN_MODELS: &[&str] = &[
    "auto",
    "composer-2",
    "composer-2-fast",
    "composer-2.5",
    "composer-2.5-fast",
];

pub const CURSOR_AGENT_DOC_URL: &str = "https://docs.cursor.com/en/cli/overview";

const CURSOR_AGENT_LIST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, serde::Serialize)]
pub struct CursorAgentProvider {
    command: PathBuf,
    #[serde(skip)]
    name: String,
}

impl CursorAgentProvider {
    pub async fn from_env(
        _tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> Result<Self> {
        let config = crate::config::Config::global();
        let command: String = config.get_cursor_agent_command().unwrap_or_default().into();
        let resolved_command = SearchPaths::builder().with_npm().resolve(&command)?;

        Ok(Self {
            command: resolved_command,
            name: CURSOR_AGENT_PROVIDER_NAME.to_string(),
        })
    }

    /// Get authentication status from cursor-agent
    async fn get_authentication_status(&self) -> bool {
        Command::new(&self.command)
            .arg("status")
            .output()
            .await
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains("✓ Logged in as"))
            .unwrap_or(false)
    }

    fn prepare_cli_command(&self) -> Command {
        let mut cmd = Command::new(&self.command);
        configure_subprocess(&mut cmd);
        if let Ok(path) = SearchPaths::builder().with_npm().path() {
            cmd.env("PATH", path);
        }
        cmd
    }

    async fn list_models_from_cli(&self) -> Result<Vec<String>, ProviderError> {
        // Prefer the dedicated `models` subcommand; fall back to `--list-models`.
        for args in [&["models"][..], &["--list-models"][..]] {
            let mut cmd = self.prepare_cli_command();
            cmd.args(args);
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            cmd.kill_on_drop(true);

            let output = match tokio::time::timeout(CURSOR_AGENT_LIST_TIMEOUT, cmd.output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(ProviderError::RequestFailed(format!(
                        "Failed to spawn cursor-agent for model listing: {e}"
                    )));
                }
                Err(_) => {
                    tracing::debug!(
                        args = ?args,
                        timeout_secs = CURSOR_AGENT_LIST_TIMEOUT.as_secs(),
                        "cursor-agent model listing timed out"
                    );
                    continue;
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !output.status.success() {
                tracing::debug!(
                    args = ?args,
                    status = ?output.status.code(),
                    stderr = %stderr,
                    "cursor-agent model listing command failed"
                );
                continue;
            }

            let models = parse_cursor_agent_models_output(&stdout);
            if !models.is_empty() {
                return Ok(models);
            }

            if !stdout.trim().is_empty() {
                tracing::debug!(
                    args = ?args,
                    stdout = %stdout,
                    "cursor-agent model listing returned no parseable models"
                );
            }
        }

        Ok(Vec::new())
    }

    /// Convert goose messages to a simple prompt format for cursor-agent CLI
    fn messages_to_cursor_agent_format(&self, system: &str, messages: &[Message]) -> String {
        let mut full_prompt = String::new();

        let filtered_system = filter_extensions_from_system_prompt(system);
        full_prompt.push_str(&filtered_system);
        full_prompt.push_str("\n\n");

        // Add conversation history
        for message in messages {
            let role_prefix = match message.role {
                Role::User => "Human: ",
                Role::Assistant => "Assistant: ",
            };
            full_prompt.push_str(role_prefix);

            for content in &message.content {
                match content {
                    MessageContent::Text(text_content) => {
                        full_prompt.push_str(&text_content.text);
                        full_prompt.push('\n');
                    }
                    MessageContent::ToolRequest(tool_request) => {
                        if let Ok(tool_call) = &tool_request.tool_call {
                            full_prompt.push_str(&format!(
                                "Tool Use: {} with args: {:?}\n",
                                tool_call.name, tool_call.arguments
                            ));
                        }
                    }
                    MessageContent::ToolResponse(tool_response) => {
                        if let Ok(result) = &tool_response.tool_result {
                            let content_text = result
                                .content
                                .iter()
                                .filter_map(|content| match content {
                                    rmcp::model::ContentBlock::Text(text_content) => {
                                        Some(text_content.text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<Vec<&str>>()
                                .join("\n");

                            full_prompt.push_str(&format!("Tool Result: {}\n", content_text));
                        }
                    }
                    _ => {
                        // Skip other content types for now
                    }
                }
            }
            full_prompt.push('\n');
        }

        full_prompt.push_str("Assistant: ");
        full_prompt
    }

    /// Parse the JSON response from cursor-agent CLI
    fn parse_cursor_agent_response(
        &self,
        lines: &[String],
    ) -> Result<(Message, Usage), ProviderError> {
        // Try parsing each line as a JSON object and find the one with type="result"
        for line in lines {
            if let Ok(json_value) = serde_json::from_str::<Value>(line) {
                if let Some(type_val) = json_value.get("type") {
                    if type_val == "result" {
                        let text_content = if let Some(result) = json_value.get("result") {
                            let result_str = result.as_str().unwrap_or("").to_string();

                            if result_str.is_empty() {
                                if json_value
                                    .get("is_error")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false)
                                {
                                    "Error: cursor-agent returned an error response".to_string()
                                } else {
                                    "cursor-agent completed successfully but returned no content"
                                        .to_string()
                                }
                            } else {
                                result_str
                            }
                        } else {
                            format!("Raw cursor-agent response: {}", line)
                        };

                        let message_content = vec![MessageContent::text(text_content)];
                        let response_message = Message::new(
                            Role::Assistant,
                            chrono::Utc::now().timestamp(),
                            message_content,
                        );

                        let usage = Usage::default();

                        return Ok((response_message, usage));
                    }
                }
            }
        }

        // If no valid result line found, fall back to joining all lines
        let response_text = lines.join("\n");

        let message_content = vec![MessageContent::text(response_text)];
        let response_message = Message::new(
            Role::Assistant,
            chrono::Utc::now().timestamp(),
            message_content,
        );
        let usage = Usage::default();
        Ok((response_message, usage))
    }

    async fn execute_command(
        &self,
        model: &ModelConfig,
        system: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> Result<Vec<String>, ProviderError> {
        let prompt = self.messages_to_cursor_agent_format(system, messages);

        if std::env::var("GOOSE_CURSOR_AGENT_DEBUG").is_ok() {
            println!("=== CURSOR AGENT PROVIDER DEBUG ===");
            println!("Command: {:?}", self.command);
            println!("Original system prompt length: {} chars", system.len());
            println!(
                "Filtered system prompt length: {} chars",
                filter_extensions_from_system_prompt(system).len()
            );
            println!("Full prompt: {}", prompt);
            println!("Model: {}", model.model_name);
            println!("================================");
        }

        let mut cmd = self.prepare_cli_command();
        cmd.arg("--model").arg(&model.model_name);

        cmd.arg("--print")
            .arg("--output-format")
            .arg("json")
            .arg("--force");

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| ProviderError::RequestFailed(format!(
                    "Failed to spawn cursor-agent CLI command '{:?}': {}. \
                    Make sure the cursor-agent CLI is installed and available in the configured search paths, or set CURSOR_AGENT_COMMAND in your config to the correct path.",
                    self.command, e
                )))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdin".to_string()))?;
        let prompt_write = tokio::spawn(async move {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await
        });

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdout".to_string()))?;
        let stderr = child.stderr.take();
        let stderr_drain = tokio::spawn(async move {
            let mut output = String::new();
            if let Some(mut stderr) = stderr {
                let _ = stderr.read_to_string(&mut output).await;
            }
            output
        });

        let mut reader = BufReader::new(stdout);
        let mut lines = Vec::new();
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push(trimmed.to_string());
                    }
                }
                Err(e) => {
                    return Err(ProviderError::RequestFailed(format!(
                        "Failed to read output: {}",
                        e
                    )));
                }
            }
        }

        let exit_status = child.wait().await.map_err(|e| {
            ProviderError::RequestFailed(format!("Failed to wait for command: {}", e))
        })?;
        let prompt_write_result = prompt_write.await;
        let _stderr = stderr_drain.await.unwrap_or_default();

        if !exit_status.success() {
            if !self.get_authentication_status().await {
                return Err(ProviderError::Authentication(
                    "You are not logged in to cursor-agent. Please run 'cursor-agent login' to authenticate first."
                        .to_string()));
            }
            return Err(ProviderError::RequestFailed(format!(
                "Command failed with exit code: {:?}",
                exit_status.code()
            )));
        }

        prompt_write_result
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to write prompt to stdin: {e}"))
            })?
            .map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to write prompt to stdin: {e}"))
            })?;

        tracing::debug!("Command executed successfully, got {} lines", lines.len());
        for (i, line) in lines.iter().enumerate() {
            tracing::debug!("Line {}: {}", i, line);
        }

        Ok(lines)
    }
}

fn static_known_models() -> Vec<String> {
    CURSOR_AGENT_KNOWN_MODELS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// Parse `cursor-agent models` / `--list-models` human-readable output.
fn parse_cursor_agent_models_output(stdout: &str) -> Vec<String> {
    let mut models = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for raw_line in stdout.lines() {
        let line = strip_ansi(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("available models")
            || lower.starts_with("no models available")
            || lower.starts_with("tip:")
            || lower.starts_with("failed to load models")
        {
            continue;
        }

        // Lines look like: "<id> - <display name> (current, default)"
        let candidate = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|c: char| c == '-' || c == ':' || c == ',' || c == '(' || c == ')');

        if candidate.is_empty() || !is_plausible_model_id(candidate) {
            continue;
        }

        if seen.insert(candidate.to_string()) {
            models.push(candidate.to_string());
        }
    }

    if models.is_empty() {
        return models;
    }

    // Keep auto routing available even if the CLI omits it.
    if seen.insert(CURSOR_AGENT_DEFAULT_MODEL.to_string()) {
        models.insert(0, CURSOR_AGENT_DEFAULT_MODEL.to_string());
    }

    models
}

fn is_plausible_model_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

impl goose_providers::base::ProviderDescriptor for CursorAgentProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            CURSOR_AGENT_PROVIDER_NAME,
            "Cursor Agent",
            "Execute AI models via cursor-agent CLI tool",
            CURSOR_AGENT_DEFAULT_MODEL,
            CURSOR_AGENT_KNOWN_MODELS.to_vec(),
            CURSOR_AGENT_DOC_URL,
            vec![ConfigKey::new(
                "CURSOR_AGENT_COMMAND",
                true,
                false,
                Some("cursor-agent"),
                true,
            )],
        )
        .with_setup(
            ProviderSetupMetadata::cli_agent(
                "cursor-agent",
                &["cursor-agent", "cursor_agent", "cursor"],
            )
            .with_docs_url("https://docs.cursor.com/en/cli/overview")
            .with_capabilities(true, true, true),
        )
    }
}

impl ProviderDef for CursorAgentProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(Self::from_env(tls_config))
    }
}

#[async_trait]
impl Provider for CursorAgentProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn uses_local_session_naming(&self) -> bool {
        true
    }

    fn skip_canonical_filtering(&self) -> bool {
        // Cursor model IDs are CLI/account-specific and often absent from the
        // canonical registry. Keep the live list intact for inventory/config.
        true
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        match self.list_models_from_cli().await {
            Ok(models) if !models.is_empty() => Ok(models),
            Ok(_) => {
                tracing::debug!(
                    "cursor-agent returned no models; falling back to known static models"
                );
                Ok(static_known_models())
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "failed to list models via cursor-agent; falling back to known static models"
                );
                Ok(static_known_models())
            }
        }
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let lines = self
            .execute_command(model_config, system, messages, tools)
            .await?;

        let (message, usage) = self.parse_cursor_agent_response(&lines)?;

        // Create a dummy payload for debug tracing
        let payload = json!({
            "command": self.command,
            "model": model_config.model_name,
            "system": system,
            "messages": messages.len()
        });

        let response = json!({
            "lines": lines.len(),
            "usage": usage
        });

        let mut log = start_log(model_config, &payload)?;
        log.write(&response, Some(&usage))?;

        let provider_usage = ProviderUsage::new(model_config.model_name.clone(), usage);
        Ok(stream_from_single_message(message, provider_usage))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const SENTINEL: &str = "loupe-sensitive-cursor-prompt";

    fn recording_cli(directory: &Path) -> PathBuf {
        let command = directory.join("cursor-agent-recording-shim");
        fs::write(
            &command,
            r#"#!/bin/sh
record_dir=${0%/*}
printf '%s\n' "$@" > "$record_dir/args"
cat > "$record_dir/stdin"
printf '%s\n' '{"type":"result","result":"ok"}'
"#,
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
        command
    }

    async fn assert_prompt_uses_stdin(messages: Vec<Message>) {
        let directory = tempfile::tempdir().unwrap();
        let provider = CursorAgentProvider {
            command: recording_cli(directory.path()),
            name: CURSOR_AGENT_PROVIDER_NAME.to_string(),
        };

        let lines = provider
            .execute_command(
                &ModelConfig::new(CURSOR_AGENT_DEFAULT_MODEL),
                "system instructions",
                &messages,
                &[],
            )
            .await
            .unwrap();

        assert_eq!(lines, vec![r#"{"type":"result","result":"ok"}"#]);
        let args = fs::read_to_string(directory.path().join("args")).unwrap();
        let stdin = fs::read_to_string(directory.path().join("stdin")).unwrap();
        assert!(!args.contains(SENTINEL));
        assert!(stdin.contains(SENTINEL));
        assert!(!args.lines().any(|arg| arg == "-p"));
        assert!(args.contains("--model\nauto"));
        assert!(args.lines().any(|arg| arg == "--print"));
        assert!(args.contains("--output-format\njson"));
        assert!(args.contains("--force"));
    }

    #[tokio::test]
    async fn initial_prompt_is_sent_on_stdin() {
        assert_prompt_uses_stdin(vec![Message::user().with_text(SENTINEL)]).await;
    }

    #[tokio::test]
    async fn resumed_conversation_is_sent_on_stdin() {
        assert_prompt_uses_stdin(vec![
            Message::user().with_text("first turn"),
            Message::assistant().with_text("first response"),
            Message::user().with_text(SENTINEL),
        ])
        .await;
    }

    #[test]
    fn session_naming_is_local() {
        let provider = CursorAgentProvider {
            command: PathBuf::from("cursor-agent"),
            name: CURSOR_AGENT_PROVIDER_NAME.to_string(),
        };

        assert!(provider.uses_local_session_naming());
    }

    #[tokio::test]
    async fn former_session_title_phrase_reaches_cli() {
        let directory = tempfile::tempdir().unwrap();
        let provider = CursorAgentProvider {
            command: recording_cli(directory.path()),
            name: CURSOR_AGENT_PROVIDER_NAME.to_string(),
        };

        let (message, _) = provider
            .complete(
                &ModelConfig::new(CURSOR_AGENT_DEFAULT_MODEL),
                "answer in four words or less",
                &[Message::user().with_text("ordinary request")],
                &[],
            )
            .await
            .unwrap();

        assert_eq!(message.as_concat_text(), "ok");
        assert!(fs::read_to_string(directory.path().join("stdin"))
            .unwrap()
            .contains("answer in four words or less"));
    }

    #[test]
    fn parse_models_output_extracts_ids_and_preserves_auto() {
        let stdout = r#"
Available models

auto - Auto
composer-2-fast - Composer 2 Fast (current, default)
gpt-5 - GPT-5
sonnet-4 - Claude Sonnet 4
sonnet-4-thinking - Claude Sonnet 4 Thinking
"#;
        let models = parse_cursor_agent_models_output(stdout);
        assert_eq!(
            models,
            vec![
                "auto".to_string(),
                "composer-2-fast".to_string(),
                "gpt-5".to_string(),
                "sonnet-4".to_string(),
                "sonnet-4-thinking".to_string(),
            ]
        );
    }

    #[test]
    fn parse_models_output_inserts_auto_when_missing() {
        let stdout = "composer-2 - Composer 2\ngpt-5 - GPT-5\n";
        let models = parse_cursor_agent_models_output(stdout);
        assert_eq!(models.first().map(String::as_str), Some("auto"));
        assert!(models.iter().any(|m| m == "composer-2"));
        assert!(models.iter().any(|m| m == "gpt-5"));
    }

    #[test]
    fn parse_models_output_ignores_status_and_tip_lines() {
        let stdout = "No models available for this account.
Tip: use --model <id> to switch.
";
        let models = parse_cursor_agent_models_output(stdout);
        assert!(models.is_empty());
    }

    #[test]
    fn parse_models_output_strips_ansi_codes() {
        let stdout = "\u{1b}[36mcomposer-2-fast\u{1b}[39m - Composer 2 Fast\n";
        let models = parse_cursor_agent_models_output(stdout);
        assert!(models.iter().any(|m| m == "composer-2-fast"));
    }
}
