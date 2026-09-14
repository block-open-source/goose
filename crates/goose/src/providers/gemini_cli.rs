use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::base::{MessageStream, Provider, ProviderDef, ProviderMetadata};
use super::cli_common::{error_from_event, extract_usage_tokens};
use super::utils::filter_extensions_from_system_prompt;
use crate::config::search_path::SearchPaths;
use crate::config::Config;
use crate::conversation::message::{Message, MessageContent};
use crate::providers::base::ConfigKey;
use crate::subprocess::configure_subprocess;
use async_stream::try_stream;
use futures::future::BoxFuture;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use rmcp::model::Role;
use rmcp::model::Tool;

const GEMINI_CLI_PROVIDER_NAME: &str = "gemini-cli";
pub const GEMINI_CLI_DEFAULT_MODEL: &str = "gemini-2.5-pro";
pub const GEMINI_CLI_KNOWN_MODELS: &[&str] = &[
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
];

pub const GEMINI_CLI_DOC_URL: &str = "https://ai.google.dev/gemini-api/docs";

#[derive(Debug, serde::Serialize)]
pub struct GeminiCliProvider {
    command: PathBuf,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    cli_session_id: Arc<OnceLock<String>>,
}

struct GeminiCliProcess {
    child: tokio::process::Child,
    reader: BufReader<tokio::process::ChildStdout>,
    prompt_write: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl GeminiCliProvider {
    pub async fn from_env(
        _tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> Result<Self> {
        let config = Config::global();
        let command: String = config.get_gemini_cli_command().unwrap_or_default().into();
        let resolved_command = SearchPaths::builder().with_npm().resolve(&command)?;

        Ok(Self {
            command: resolved_command,
            name: GEMINI_CLI_PROVIDER_NAME.to_string(),
            cli_session_id: Arc::new(OnceLock::new()),
        })
    }

    fn session_id(&self) -> Option<&str> {
        self.cli_session_id.get().map(|s| s.as_str())
    }

    fn last_user_message_text(messages: &[Message]) -> String {
        messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.as_concat_text())
            .unwrap_or_default()
    }

    /// Build the prompt for the CLI invocation. When resuming a session the CLI
    /// maintains conversation context internally, so only the latest user
    /// message is needed. On the first turn (no session yet) the system prompt
    /// is prepended — there is typically only one user message at that point.
    fn build_prompt(&self, system: &str, messages: &[Message]) -> String {
        let user_text = Self::last_user_message_text(messages);

        if self.session_id().is_some() {
            user_text
        } else {
            let filtered_system = filter_extensions_from_system_prompt(system);
            if filtered_system.is_empty() {
                user_text
            } else {
                format!("{filtered_system}\n\n{user_text}")
            }
        }
    }

    fn build_command(&self, model_name: &str) -> Command {
        let mut cmd = Command::new(&self.command);
        configure_subprocess(&mut cmd);

        if let Ok(path) = SearchPaths::builder().with_npm().path() {
            cmd.env("PATH", path);
        }

        cmd.arg("-m").arg(model_name);

        if let Some(sid) = self.session_id() {
            cmd.arg("-r").arg(sid);
        }

        cmd.arg("--output-format").arg("stream-json").arg("--yolo");

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        cmd
    }

    fn spawn_command(
        &self,
        system: &str,
        messages: &[Message],
        model_name: &str,
    ) -> Result<GeminiCliProcess, ProviderError> {
        let prompt = self.build_prompt(system, messages);

        tracing::debug!(command = ?self.command, "Executing Gemini CLI command");

        let mut cmd = self.build_command(model_name);

        let mut child = cmd.kill_on_drop(true).spawn().map_err(|e| {
            ProviderError::RequestFailed(format!(
                "Failed to spawn Gemini CLI command '{}': {e}. \
                Make sure the Gemini CLI is installed and available in the configured search paths.",
                self.command.display()
            ))
        })?;

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

        Ok(GeminiCliProcess {
            child,
            reader: BufReader::new(stdout),
            prompt_write,
        })
    }
}

impl goose_providers::base::ProviderDescriptor for GeminiCliProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            GEMINI_CLI_PROVIDER_NAME,
            "Gemini CLI",
            "[Deprecated: use the Google or Vertex AI provider instead] Execute Gemini models via gemini CLI tool. Requires gemini CLI installed.",
            GEMINI_CLI_DEFAULT_MODEL,
            GEMINI_CLI_KNOWN_MODELS.to_vec(),
            GEMINI_CLI_DOC_URL,
            vec![ConfigKey::new(
                "GEMINI_CLI_COMMAND",
                true,
                false,
                Some("gemini"),
                true,
            )],
        )
        .deprecated(Some("gemini_oauth"))
    }
}

impl ProviderDef for GeminiCliProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<crate::config::ExtensionConfig>,
        tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(Self::from_env(tls_config))
    }
}

#[async_trait]
impl Provider for GeminiCliProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn manages_own_context(&self) -> bool {
        true
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(GEMINI_CLI_KNOWN_MODELS
            .iter()
            .map(|s| s.to_string())
            .collect())
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let GeminiCliProcess {
            mut child,
            mut reader,
            prompt_write,
        } = self.spawn_command(system, messages, &model_config.model_name)?;
        let session_id_lock = Arc::clone(&self.cli_session_id);
        let model_name = model_config.model_name.clone();
        let message_id = uuid::Uuid::new_v4().to_string();

        let stderr = child.stderr.take();
        let stderr_drain = tokio::spawn(async move {
            let mut buf = String::new();
            if let Some(mut stderr) = stderr {
                let _ = AsyncReadExt::read_to_string(&mut stderr, &mut buf).await;
            }
            buf
        });

        Ok(Box::pin(try_stream! {
            let mut line = String::new();
            let mut accumulated_usage = Usage::default();
            let stream_timestamp = chrono::Utc::now().timestamp();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("init") => {
                                    if let Some(sid) =
                                        parsed.get("session_id").and_then(|s| s.as_str())
                                    {
                                        let _ = session_id_lock.set(sid.to_string());
                                    }
                                }
                                Some("message") => {
                                    let is_assistant = parsed.get("role").and_then(|r| r.as_str())
                                        == Some("assistant");
                                    let content = parsed
                                        .get("content")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("");
                                    if is_assistant && !content.is_empty() {
                                        let mut partial = Message::new(
                                            Role::Assistant,
                                            stream_timestamp,
                                            vec![MessageContent::text(content)],
                                        );
                                        partial.id = Some(message_id.clone());
                                        yield (Some(partial), None);
                                    }
                                }
                                Some("result") => {
                                    if let Some(stats) = parsed.get("stats") {
                                        accumulated_usage = extract_usage_tokens(stats);
                                    }
                                    break;
                                }
                                Some("error") => {
                                    let _ = child.wait().await;
                                    Err(error_from_event("Gemini CLI", &parsed))?;
                                }
                                _ => {}
                            }
                        } else {
                            tracing::warn!(line = trimmed, "Non-JSON line in stream-json output");
                        }
                    }
                    Err(e) => {
                        let _ = child.wait().await;
                        Err(ProviderError::RequestFailed(format!(
                            "Failed to read streaming output: {e}"
                        )))?;
                    }
                }
            }

            let prompt_write_result = prompt_write.await;
            let stderr_text = stderr_drain.await.unwrap_or_default();
            let exit_status = child.wait().await.map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to wait for command: {e}"))
            })?;

            if !exit_status.success() {
                let stderr_snippet = stderr_text.trim();
                let detail = if stderr_snippet.is_empty() {
                    format!("exit code {:?}", exit_status.code())
                } else {
                    format!("exit code {:?}: {stderr_snippet}", exit_status.code())
                };
                Err(ProviderError::RequestFailed(format!(
                    "Gemini CLI command failed ({detail})"
                )))?;
            }

            prompt_write_result
                .map_err(|e| ProviderError::RequestFailed(format!(
                    "Failed to write prompt to stdin: {e}"
                )))?
                .map_err(|e| ProviderError::RequestFailed(format!(
                    "Failed to write prompt to stdin: {e}"
                )))?;

            let provider_usage = ProviderUsage::new(model_name, accumulated_usage);
            yield (None, Some(provider_usage));
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use futures::StreamExt;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::Path;

    #[cfg(unix)]
    const SENTINEL: &str = "loupe-sensitive-gemini-prompt";

    fn make_provider() -> GeminiCliProvider {
        GeminiCliProvider {
            command: PathBuf::from("gemini"),
            name: "gemini-cli".to_string(),
            cli_session_id: Arc::new(OnceLock::new()),
        }
    }

    #[test]
    fn test_build_prompt_first_and_resume() {
        let provider = make_provider();
        let messages = vec![Message::new(
            Role::User,
            0,
            vec![MessageContent::text("Hello")],
        )];

        let prompt = provider.build_prompt("You are helpful.", &messages);
        assert!(prompt.contains("You are helpful."));
        assert!(prompt.contains("Hello"));

        let _ = provider.cli_session_id.set("session-123".to_string());
        let messages = vec![
            Message::new(Role::User, 0, vec![MessageContent::text("Hello")]),
            Message::new(Role::Assistant, 0, vec![MessageContent::text("Hi!")]),
            Message::new(
                Role::User,
                0,
                vec![MessageContent::text("Follow up question")],
            ),
        ];
        let prompt = provider.build_prompt("You are helpful.", &messages);
        assert_eq!(prompt, "Follow up question");
    }

    #[cfg(unix)]
    fn recording_cli(directory: &Path) -> PathBuf {
        let command = directory.join("gemini-recording-shim");
        fs::write(
            &command,
            r#"#!/bin/sh
record_dir=${0%/*}
printf '%s\n' "$@" > "$record_dir/args"
cat > "$record_dir/stdin"
printf '%s\n' '{"type":"init","session_id":"recorded-session"}'
printf '%s\n' '{"type":"result","stats":{}}'
"#,
        )
        .unwrap();
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
        command
    }

    #[cfg(unix)]
    async fn assert_prompt_uses_stdin(resumed: bool) {
        let directory = tempfile::tempdir().unwrap();
        let mut provider = make_provider();
        provider.command = recording_cli(directory.path());
        if resumed {
            provider
                .cli_session_id
                .set("existing-session".to_string())
                .unwrap();
        }

        let messages = if resumed {
            vec![
                Message::user().with_text("first turn"),
                Message::assistant().with_text("first response"),
                Message::user().with_text(SENTINEL),
            ]
        } else {
            vec![Message::user().with_text(SENTINEL)]
        };
        let mut stream = provider
            .stream(
                &ModelConfig::new(GEMINI_CLI_DEFAULT_MODEL),
                "system instructions",
                &messages,
                &[],
            )
            .await
            .unwrap();
        while let Some(item) = stream.next().await {
            item.unwrap();
        }

        let args = fs::read_to_string(directory.path().join("args")).unwrap();
        let stdin = fs::read_to_string(directory.path().join("stdin")).unwrap();
        assert!(!args.contains(SENTINEL));
        assert!(stdin.contains(SENTINEL));
        assert!(!args.lines().any(|arg| arg == "-p"));
        assert!(args.contains("-m\ngemini-2.5-pro"));
        assert!(args.contains("--output-format\nstream-json"));
        assert!(args.contains("--yolo"));
        if resumed {
            assert!(args.contains("-r\nexisting-session"));
        } else {
            assert!(!args.lines().any(|arg| arg == "-r"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn initial_prompt_is_sent_on_stdin() {
        assert_prompt_uses_stdin(false).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resumed_prompt_is_sent_on_stdin() {
        assert_prompt_uses_stdin(true).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn former_session_title_phrase_reaches_cli() {
        let directory = tempfile::tempdir().unwrap();
        let mut provider = make_provider();
        provider.command = recording_cli(directory.path());

        let mut stream = provider
            .stream(
                &ModelConfig::new(GEMINI_CLI_DEFAULT_MODEL),
                "answer in four words or less",
                &[Message::user().with_text("ordinary request")],
                &[],
            )
            .await
            .unwrap();
        while let Some(item) = stream.next().await {
            item.unwrap();
        }

        assert!(fs::read_to_string(directory.path().join("stdin"))
            .unwrap()
            .contains("answer in four words or less"));
    }
}
