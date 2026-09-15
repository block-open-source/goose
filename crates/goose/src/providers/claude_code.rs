use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use futures::future::BoxFuture;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use rmcp::model::{Role, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

use super::base::{
    ConfigKey, MessageStream, PermissionRouting, Provider, ProviderDef, ProviderMetadata,
};
use super::utils::filter_extensions_from_system_prompt;
use crate::config::paths::Paths;
use crate::config::search_path::SearchPaths;
use crate::config::{Config, ExtensionConfig, GooseMode};
use crate::conversation::message::{Message, MessageContent};
use crate::permission::permission_confirmation::PrincipalType;
use crate::permission::{Permission, PermissionConfirmation};
use crate::subprocess::configure_subprocess;
use goose_providers::model::ModelConfig;

use super::cli_common::{error_from_event, extract_usage_tokens};
use super::private_file::create_private_named_temp_file;

const CLAUDE_CODE_PROVIDER_NAME: &str = "claude-code";
pub const CLAUDE_CODE_DEFAULT_MODEL: &str = "default";
pub const CLAUDE_CODE_DOC_URL: &str = "https://code.claude.com/docs/en/setup";

// https://github.com/anthropics/claude-agent-sdk-python/blob/0e9397e/src/claude_agent_sdk/types.py#L857-L859
#[derive(Serialize)]
struct ControlResponse<T: Serialize> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    response: ControlResponseBody<T>,
}

#[derive(Serialize)]
struct ControlResponseBody<T: Serialize> {
    subtype: &'static str,
    request_id: String,
    response: T,
}

// https://github.com/anthropics/claude-agent-sdk-python/blob/0e9397e/src/claude_agent_sdk/types.py#L135-L153
#[derive(Serialize)]
#[serde(tag = "behavior")]
enum PermissionResponse {
    #[serde(rename = "allow")]
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: serde_json::Map<String, Value>,
        #[serde(rename = "toolUseID")]
        tool_use_id: String,
    },
    #[serde(rename = "deny")]
    Deny { message: String },
}

#[derive(Serialize)]
struct ControlRequest {
    #[serde(rename = "type")]
    msg_type: &'static str,
    request_id: String,
    request: ControlRequestBody,
}

#[derive(Serialize)]
#[serde(tag = "subtype")]
enum ControlRequestBody {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "set_model")]
    SetModel { model: String },
}

impl ControlRequestBody {
    fn label(&self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::SetModel { .. } => "set_model",
        }
    }
}

#[derive(Deserialize)]
struct IncomingControlResponse {
    response: IncomingControlResponseBody,
}

#[derive(Deserialize)]
#[serde(tag = "subtype")]
enum IncomingControlResponseBody {
    #[serde(rename = "success")]
    Success {
        request_id: String,
        #[serde(default)]
        response: Option<Value>,
    },
    #[serde(rename = "error")]
    Error {
        request_id: String,
        #[serde(default)]
        error: String,
    },
}

#[derive(Deserialize)]
struct IncomingControlRequest {
    request_id: String,
    request: IncomingRequestBody,
}

#[derive(Deserialize)]
#[serde(tag = "subtype")]
enum IncomingRequestBody {
    #[serde(rename = "can_use_tool")]
    CanUseTool {
        tool_name: String,
        #[serde(default)]
        input: serde_json::Map<String, Value>,
        #[serde(default)]
        tool_use_id: String,
    },
}

impl<T: Serialize> ControlResponse<T> {
    fn success(request_id: String, response: T) -> Self {
        Self {
            msg_type: "control_response",
            response: ControlResponseBody {
                subtype: "success",
                request_id,
                response,
            },
        }
    }
}

struct CliProcess {
    child: tokio::process::Child,
    stdin: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
    reader: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    #[allow(dead_code)]
    stderr_handle: tokio::task::JoinHandle<String>,
    current_model: String,
    log_model_update: bool,
    next_request_id: u64,
    needs_drain: bool,
    _system_prompt_file: NamedTempFile,
}

impl std::fmt::Debug for CliProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliProcess")
            .field("current_model", &self.current_model)
            .field("next_request_id", &self.next_request_id)
            .finish_non_exhaustive()
    }
}

impl CliProcess {
    fn next_request_id(&mut self) -> String {
        let id = self.next_request_id;
        self.next_request_id += 1;
        format!("req_{id}")
    }

    async fn send_control_request(
        &mut self,
        body: ControlRequestBody,
    ) -> Result<Option<Value>, ProviderError> {
        let request_id = self.next_request_id();
        exchange_control(&mut self.stdin, &mut self.reader, &request_id, body).await
    }

    async fn send_set_model(&mut self, model: &str) -> Result<(), ProviderError> {
        if model == self.current_model {
            return Ok(());
        }
        self.send_control_request(ControlRequestBody::SetModel {
            model: model.to_string(),
        })
        .await?;
        self.current_model = model.to_string();
        self.log_model_update = true;
        Ok(())
    }

    async fn drain_pending_response(&mut self) {
        if !self.needs_drain {
            return;
        }
        tracing::debug!("Draining cancelled response from CLI process");

        let drain = async {
            let mut line = String::new();
            loop {
                line.clear();
                match self.reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("result") | Some("error") => break,
                                _ => continue,
                            }
                        } else {
                            tracing::trace!(line = trimmed, "Non-JSON line during drain");
                        }
                    }
                    Err(_) => break,
                }
            }
        };

        const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
        if tokio::time::timeout(DRAIN_TIMEOUT, drain).await.is_err() {
            // CLI is still producing the old response. Leave needs_drain
            // true so the next call retries — by then the old response
            // likely completed and drain will succeed quickly.
            tracing::warn!(
                "Drain did not complete in {DRAIN_TIMEOUT:?}; \
                 will retry on next request"
            );
            return;
        }

        self.needs_drain = false;
        tracing::debug!("Drain complete, protocol re-synced");
    }
}

impl Drop for CliProcess {
    fn drop(&mut self) {
        self.stderr_handle.abort();
        let _ = self.child.start_kill();
    }
}

/// Spawns the Claude Code CLI (`claude`) as a persistent child process using
/// `--input-format stream-json --output-format stream-json`. The CLI stays alive
/// across turns, maintaining conversation state internally. Messages are sent as
/// NDJSON on stdin with content arrays supporting text and image blocks. Responses
/// are NDJSON on stdout (`assistant` + `result` events per turn).
#[derive(Debug, serde::Serialize)]
pub struct ClaudeCodeProvider {
    command: PathBuf,
    #[serde(skip)]
    name: String,
    /// Temp file holding MCP config JSON (auto-deleted on drop).
    #[serde(skip)]
    mcp_config_file: Option<NamedTempFile>,
    #[serde(skip)]
    cli_process: tokio::sync::OnceCell<Arc<tokio::sync::Mutex<CliProcess>>>,
    #[serde(skip)]
    pending_confirmations:
        Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<PermissionConfirmation>>>>,
    #[serde(skip)]
    initial_mode: tokio::sync::Mutex<Option<GooseMode>>,
}

impl ClaudeCodeProvider {
    /// Build content blocks from the last user message only. The CLI maintains
    /// conversation context internally per session_id.
    fn last_user_content_blocks(&self, messages: &[Message]) -> Vec<Value> {
        let msgs = match messages.iter().rev().find(|m| m.role == Role::User) {
            Some(msg) => std::slice::from_ref(msg),
            None => messages,
        };
        let mut blocks: Vec<Value> = Vec::new();
        for message in msgs {
            let prefix = match message.role {
                Role::User => "Human: ",
                Role::Assistant => "Assistant: ",
            };
            let mut text_parts = Vec::new();
            for content in &message.content {
                match content {
                    MessageContent::Text(t) => text_parts.push(t.text.clone()),
                    MessageContent::Image(img) => {
                        if !text_parts.is_empty() {
                            blocks.push(json!({"type":"text","text":format!("{}{}", prefix, text_parts.join("\n"))}));
                            text_parts.clear();
                        }
                        blocks.push(json!({"type":"image","source":{"type":"base64","media_type":img.mime_type,"data":img.data}}));
                    }
                    MessageContent::ToolRequest(req) => {
                        if let Ok(call) = &req.tool_call {
                            text_parts.push(format!("[tool_use: {} id={}]", call.name, req.id));
                        }
                    }
                    MessageContent::ToolResponse(resp) => {
                        if let Ok(result) = &resp.tool_result {
                            let text: String = result
                                .content
                                .iter()
                                .filter_map(|c| match c {
                                    rmcp::model::ContentBlock::Text(t) => Some(t.text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<&str>>()
                                .join("\n");
                            text_parts.push(format!("[tool_result id={}] {}", resp.id, text));
                        }
                    }
                    _ => {}
                }
            }
            if !text_parts.is_empty() {
                blocks.push(
                    json!({"type":"text","text":format!("{}{}", prefix, text_parts.join("\n"))}),
                );
            }
        }
        blocks
    }

    fn build_stream_json_command(&self) -> Command {
        let mut cmd = Command::new(&self.command);
        configure_subprocess(&mut cmd);
        // Allow goose to run inside a Claude Code session.
        cmd.env_remove("CLAUDECODE");
        cmd.arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Returns true when the control protocol is enabled.
    fn apply_permission_flags(cmd: &mut Command) -> Result<bool, ProviderError> {
        let config = Config::global();
        let goose_mode = config.get_goose_mode().unwrap_or(GooseMode::Auto);

        match goose_mode {
            GooseMode::Auto => {
                cmd.arg("--dangerously-skip-permissions");
                Ok(false)
            }
            GooseMode::SmartApprove | GooseMode::Approve => {
                cmd.arg("--permission-prompt-tool").arg("stdio");
                Ok(true)
            }
            GooseMode::Chat => Ok(false),
        }
    }

    fn apply_system_prompt_file(
        cmd: &mut Command,
        state_dir: &Path,
        filtered_system: &str,
    ) -> Result<NamedTempFile, ProviderError> {
        let system_prompt_file =
            write_system_prompt_file(state_dir, filtered_system).map_err(|e| {
                ProviderError::RequestFailed(format!(
                    "Failed to create Claude CLI system prompt file: {e}"
                ))
            })?;
        cmd.arg("--system-prompt-file")
            .arg(system_prompt_file.path());
        Ok(system_prompt_file)
    }

    async fn spawn_process(
        &self,
        model: &ModelConfig,
        filtered_system: &str,
    ) -> Result<CliProcess, ProviderError> {
        let mut cmd = self.build_stream_json_command();

        if let Some(f) = &self.mcp_config_file {
            cmd.arg("--mcp-config").arg(f.path());
            cmd.arg("--strict-mcp-config");
        }

        cmd.arg("--include-partial-messages");
        let system_prompt_file =
            Self::apply_system_prompt_file(&mut cmd, &Paths::state_dir(), filtered_system)?;
        cmd.arg("--model").arg(&model.model_name);

        let control_protocol_enabled = Self::apply_permission_flags(&mut cmd)?;

        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::RequestFailed(format!(
                "Failed to spawn Claude CLI command '{:?}': {}.",
                self.command, e
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdout".to_string()))?;

        let stderr = child.stderr.take();
        let stderr_handle = tokio::spawn(async move {
            let mut output = String::new();
            if let Some(mut stderr) = stderr {
                use tokio::io::AsyncReadExt;
                let _ = stderr.read_to_string(&mut output).await;
            }
            output
        });

        let mut process = CliProcess {
            child,
            stdin: Box::new(stdin),
            reader: BufReader::new(Box::new(stdout)),
            stderr_handle,
            current_model: model.model_name.clone(),
            log_model_update: false,
            next_request_id: 0,
            needs_drain: false,
            _system_prompt_file: system_prompt_file,
        };

        if control_protocol_enabled {
            process
                .send_control_request(ControlRequestBody::Initialize)
                .await?;
        }

        Ok(process)
    }

    async fn get_or_init_process(
        &self,
        model_config: &ModelConfig,
        filtered_system: &str,
    ) -> Result<&Arc<tokio::sync::Mutex<CliProcess>>, ProviderError> {
        self.cli_process
            .get_or_try_init(|| async {
                Ok(Arc::new(tokio::sync::Mutex::new(
                    self.spawn_process(model_config, filtered_system).await?,
                )))
            })
            .await
    }
}

async fn exchange_control(
    stdin: &mut (impl AsyncWrite + Unpin),
    reader: &mut (impl AsyncBufRead + Unpin),
    request_id: &str,
    body: ControlRequestBody,
) -> Result<Option<Value>, ProviderError> {
    let label = body.label();
    let req = ControlRequest {
        msg_type: "control_request",
        request_id: request_id.to_string(),
        request: body,
    };
    let mut req_str = serde_json::to_string(&req).map_err(|e| {
        ProviderError::RequestFailed(format!("Failed to serialize {label} request: {e}"))
    })?;
    req_str.push('\n');
    stdin.write_all(req_str.as_bytes()).await.map_err(|e| {
        ProviderError::RequestFailed(format!("Failed to write {label} request: {e}"))
    })?;

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                return Err(ProviderError::RequestFailed(format!(
                    "CLI process terminated while waiting for {label} response"
                )));
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(msg) = serde_json::from_str::<IncomingControlResponse>(trimmed) {
                    match msg.response {
                        IncomingControlResponseBody::Success {
                            request_id: ref rid,
                            response,
                        } if rid == request_id => return Ok(response),
                        IncomingControlResponseBody::Error {
                            request_id: ref rid,
                            error,
                        } if rid == request_id => {
                            return Err(ProviderError::RequestFailed(format!(
                                "{label} failed: {error}"
                            )));
                        }
                        _ => continue,
                    }
                }
            }
            Err(e) => {
                return Err(ProviderError::RequestFailed(format!(
                    "Failed to read {label} response: {e}"
                )));
            }
        }
    }
}

fn extract_model_aliases(response: Option<&Value>) -> Vec<String> {
    response
        .and_then(|v| v.get("models")?.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("value")?.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn build_stream_json_input(content_blocks: &[Value], session_id: &str) -> String {
    let msg = json!({"type":"user","session_id":session_id,"message":{"role":"user","content":content_blocks}});
    serde_json::to_string(&msg).expect("serializing JSON content blocks cannot fail")
}

fn claude_mcp_config_json(extensions: &[ExtensionConfig]) -> Option<String> {
    let mut mcp_servers = serde_json::Map::new();

    for extension in extensions {
        match extension {
            ExtensionConfig::StreamableHttp { uri, headers, .. } => {
                let key = extension.key();
                let mut config = serde_json::Map::new();
                config.insert("type".to_string(), json!("http"));
                config.insert("url".to_string(), json!(uri));
                if !headers.is_empty() {
                    config.insert("headers".to_string(), json!(headers));
                }
                mcp_servers.insert(key, Value::Object(config));
            }
            ExtensionConfig::Stdio {
                cmd, args, envs, ..
            } => {
                let key = extension.key();
                let mut config = serde_json::Map::new();
                config.insert("type".to_string(), json!("stdio"));
                config.insert("command".to_string(), json!(cmd));
                if !args.is_empty() {
                    config.insert("args".to_string(), json!(args));
                }
                let env_map = envs.get_env();
                if !env_map.is_empty() {
                    config.insert("env".to_string(), json!(env_map));
                }
                mcp_servers.insert(key, Value::Object(config));
            }
            _ => {}
        }
    }

    if mcp_servers.is_empty() {
        return None;
    }

    serde_json::to_string(&json!({ "mcpServers": mcp_servers })).ok()
}

fn write_claude_temp_file(
    state_dir: &Path,
    prefix: &str,
    suffix: &str,
    contents: &str,
) -> Result<NamedTempFile, anyhow::Error> {
    let dir = state_dir.join("claude-code");
    std::fs::create_dir_all(&dir)?;
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix).suffix(suffix);
    let mut tmp = create_private_named_temp_file(&mut builder, &dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    tmp.write_all(contents.as_bytes())?;
    Ok(tmp)
}

/// Write the MCP config JSON to a temp file with restricted permissions
/// so secrets (headers, env vars) are not leaked via process argv.
fn write_mcp_config_file(state_dir: &Path, json: &str) -> Result<NamedTempFile, anyhow::Error> {
    let prefix = format!("mcp-config-{}_", chrono::Utc::now().format("%Y%m%d"));
    write_claude_temp_file(state_dir, &prefix, ".json", json)
}

fn write_system_prompt_file(
    state_dir: &Path,
    system_prompt: &str,
) -> Result<NamedTempFile, anyhow::Error> {
    let prefix = format!("system-prompt-{}_", chrono::Utc::now().format("%Y%m%d"));
    write_claude_temp_file(state_dir, &prefix, ".txt", system_prompt)
}

impl goose_providers::base::ProviderDescriptor for ClaudeCodeProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            CLAUDE_CODE_PROVIDER_NAME,
            "Claude Code CLI",
            "[Deprecated: use claude-acp instead] Requires claude CLI installed, no MCPs. Use claude-acp for ACP support with extensions.",
            CLAUDE_CODE_DEFAULT_MODEL,
            // Only a few agentic choices; fetched dynamically via fetch_supported_models.
            vec![],
            CLAUDE_CODE_DOC_URL,
            vec![ConfigKey::new(
                "CLAUDE_CODE_COMMAND",
                true,
                false,
                Some("claude"),
                true,
            )],
        )
        .deprecated(Some("claude-acp"))
    }
}

impl ProviderDef for ClaudeCodeProvider {
    type Provider = Self;

    fn from_env(
        extensions: Vec<ExtensionConfig>,
        _tls_config: Option<crate::providers::api_client::TlsConfig>,
    ) -> BoxFuture<'static, Result<Self::Provider>> {
        Box::pin(async move {
            let config = crate::config::Config::global();
            let command: String = config.get_claude_code_command().unwrap_or_default().into();
            let resolved_command = SearchPaths::builder().with_npm().resolve(command)?;

            let mut resolved = Vec::with_capacity(extensions.len());
            for ext in extensions {
                resolved.push(ext.resolve(config).await?);
            }

            let mcp_config_file = claude_mcp_config_json(&resolved)
                .map(|json| write_mcp_config_file(&Paths::state_dir(), &json))
                .transpose()?;

            Ok(Self {
                command: resolved_command,
                name: CLAUDE_CODE_PROVIDER_NAME.to_string(),
                mcp_config_file,
                cli_process: tokio::sync::OnceCell::new(),
                pending_confirmations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
                initial_mode: tokio::sync::Mutex::new(None),
            })
        })
    }
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn manages_own_context(&self) -> bool {
        true
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        // Uses a separate short-lived process because --system-prompt-file is a CLI-only
        // flag with no NDJSON equivalent. The persistent process needs it at spawn,
        // but it's unavailable during model listing.
        // See: https://code.claude.com/docs/en/cli-reference#system-prompt-flags
        let mut cmd = self.build_stream_json_command();
        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::RequestFailed(format!("Failed to spawn CLI for model listing: {e}"))
        })?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::RequestFailed("Failed to capture stdout".to_string()))?;

        let mut reader = BufReader::new(stdout);
        let response = exchange_control(
            &mut stdin,
            &mut reader,
            "model_list",
            ControlRequestBody::Initialize,
        )
        .await;
        let _ = child.kill().await;
        Ok(extract_model_aliases(response.ok().flatten().as_ref()))
    }

    async fn update_mode(&self, _session_id: &str, mode: GooseMode) -> Result<(), ProviderError> {
        // Mode is baked into the subprocess at spawn; claude-acp replaces
        // this provider (#7801).
        let mut guard = self.initial_mode.lock().await;
        let current = *guard.get_or_insert(mode);
        if current != mode {
            return Err(ProviderError::RequestFailed(format!(
                "Mode change not supported: session is {current}, requested {mode}",
            )));
        }
        Ok(())
    }

    fn permission_routing(&self) -> PermissionRouting {
        PermissionRouting::ActionRequired
    }

    async fn handle_permission_confirmation(
        &self,
        request_id: &str,
        confirmation: &PermissionConfirmation,
    ) -> bool {
        let mut pending = self.pending_confirmations.lock().await;
        if let Some(tx) = pending.remove(request_id) {
            let _ = tx.send(confirmation.clone());
            return true;
        }
        false
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let session_id = crate::session_context::current_session_id().unwrap_or_default();
        let filtered_system = filter_extensions_from_system_prompt(system);
        let process_arc = Arc::clone(
            self.get_or_init_process(model_config, &filtered_system)
                .await?,
        );

        // Prepare the payload outside the lock — these don't need the process.
        let blocks = self.last_user_content_blocks(messages);
        let ndjson_line = build_stream_json_input(&blocks, &session_id);
        let model_name = model_config.model_name.clone();
        let mut current_text_message_id = uuid::Uuid::new_v4().to_string();
        let pending_confirmations = Arc::clone(&self.pending_confirmations);

        Ok(Box::pin(try_stream! {
            // Single lock acquisition covers write-to-stdin and read-from-stdout,
            // eliminating the race window between the two.
            let mut process = process_arc.lock_owned().await;

            // Clean up pending permissions from a cancelled stream
            {
                let mut pending = pending_confirmations.lock().await;
                for (req_id, tx) in pending.drain() {
                    drop(tx);
                    let resp = ControlResponse::success(
                        req_id,
                        PermissionResponse::Deny { message: "Stream cancelled".to_string() },
                    );
                    let mut s = serde_json::to_string(&resp).map_err(|e| {
                        ProviderError::RequestFailed(format!("Failed to serialize cleanup deny response: {e}"))
                    })?;
                    s.push('\n');
                    let _ = process.stdin.write_all(s.as_bytes()).await;
                }
            }

            process.drain_pending_response().await;
            process.send_set_model(&model_name).await?;

            process
                .stdin
                .write_all(ndjson_line.as_bytes())
                .await
                .map_err(|e| {
                    ProviderError::RequestFailed(format!("Failed to write to stdin: {}", e))
                })?;
            process.stdin.write_all(b"\n").await.map_err(|e| {
                ProviderError::RequestFailed(format!("Failed to write newline to stdin: {}", e))
            })?;

            process.needs_drain = true;
            let mut line = String::new();
            let mut accumulated_usage = Usage::default();
            let mut stream_error: Option<ProviderError> = None;
            let stream_timestamp = chrono::Utc::now().timestamp();

            loop {
                line.clear();
                match process.reader.read_line(&mut line).await {
                    Ok(0) => {
                        process.needs_drain = false;
                        stream_error = Some(ProviderError::RequestFailed(
                            "Claude CLI process terminated unexpectedly".to_string(),
                        ));
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("stream_event") => {
                                    if let Some(event) = parsed.get("event") {
                                        match event.get("type").and_then(|t| t.as_str()) {
                                            Some("content_block_delta") => {
                                                if let Some(text) = event
                                                    .get("delta")
                                                    .filter(|d| {
                                                        d.get("type").and_then(|t| t.as_str())
                                                            == Some("text_delta")
                                                    })
                                                    .and_then(|d| d.get("text"))
                                                    .and_then(|t| t.as_str())
                                                {
                                                    if !text.is_empty() {
                                                        let mut partial_message = Message::new(
                                                            Role::Assistant,
                                                            stream_timestamp,
                                                            vec![MessageContent::text(text)],
                                                        );
                                                        partial_message.id =
                                                            Some(current_text_message_id.clone());
                                                        yield (Some(partial_message), None);
                                                    }
                                                }
                                            }
                                            Some("message_start") => {
                                                if let Some(usage_info) = event
                                                    .get("message")
                                                    .and_then(|m| m.get("usage"))
                                                {
                                                    let new = extract_usage_tokens(usage_info);
                                                    if let Some(i) = new.input_tokens {
                                                        accumulated_usage.input_tokens = Some(i);
                                                        accumulated_usage.cache_read_input_tokens =
                                                            new.cache_read_input_tokens;
                                                        accumulated_usage.cache_write_input_tokens =
                                                            new.cache_write_input_tokens;
                                                    }
                                                }
                                            }
                                            Some("message_delta") => {
                                                if let Some(usage_info) = event.get("usage") {
                                                    let new = extract_usage_tokens(usage_info);
                                                    if let Some(o) = new.output_tokens {
                                                        accumulated_usage.output_tokens = Some(o);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Some("result") => {
                                    process.needs_drain = false;
                                    if parsed
                                        .get("is_error")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                    {
                                        let subtype = parsed
                                            .get("subtype")
                                            .and_then(Value::as_str)
                                            .unwrap_or("error");
                                        let mut details = Vec::new();
                                        if let Some(error) =
                                            parsed.get("error").and_then(Value::as_str)
                                        {
                                            details.push(error);
                                        }
                                        if let Some(errors) =
                                            parsed.get("errors").and_then(Value::as_array)
                                        {
                                            details.extend(errors.iter().filter_map(Value::as_str));
                                        }
                                        if let Some(result) =
                                            parsed.get("result").and_then(Value::as_str)
                                        {
                                            details.push(result);
                                        }
                                        let details = details.join("; ");
                                        let message = match (subtype, details.is_empty()) {
                                            ("success", false) => details,
                                            (_, false) => format!("{subtype}: {details}"),
                                            _ => subtype.to_string(),
                                        };
                                        stream_error = Some(ProviderError::RequestFailed(format!(
                                            "Claude CLI error: {message}"
                                        )));
                                        break;
                                    }
                                    if let Some(usage_info) = parsed.get("usage") {
                                        let new = extract_usage_tokens(usage_info);
                                        let reports_own_cache = new.cache_read_input_tokens.is_some()
                                            || new.cache_write_input_tokens.is_some();
                                        let cache_read = new
                                            .cache_read_input_tokens
                                            .or(accumulated_usage.cache_read_input_tokens);
                                        let cache_write = new
                                            .cache_write_input_tokens
                                            .or(accumulated_usage.cache_write_input_tokens);
                                        // A result with raw input but no cache breakdown
                                        // inherits the streamed breakdown; fold it back in
                                        // so input stays inclusive of cache tokens.
                                        let output_tokens =
                                            new.output_tokens.or(accumulated_usage.output_tokens);
                                        accumulated_usage = if new.input_tokens.is_some()
                                            && !reports_own_cache
                                        {
                                            Usage::from_cache_exclusive_input(
                                                new.input_tokens,
                                                output_tokens,
                                                None,
                                                cache_read,
                                                cache_write,
                                            )
                                        } else {
                                            Usage::new(
                                                new.input_tokens.or(accumulated_usage.input_tokens),
                                                output_tokens,
                                                None,
                                            )
                                            .with_cache_tokens(cache_read, cache_write)
                                        };
                                    }
                                    break;
                                }
                                Some("error") => {
                                    process.needs_drain = false;
                                    stream_error = Some(error_from_event("Claude CLI", &parsed));
                                    break;
                                }
                                Some("control_request") => {
                                    if let Ok(IncomingControlRequest {
                                        request_id,
                                        request: IncomingRequestBody::CanUseTool { tool_name, input, tool_use_id },
                                    }) = serde_json::from_str::<IncomingControlRequest>(trimmed) {
                                        tracing::debug!(raw = %parsed, "can_use_tool control_request received");

                                        let (tx, rx) = oneshot::channel();
                                        pending_confirmations.lock().await.insert(request_id.clone(), tx);

                                        let action_msg = Message::assistant().with_action_required(
                                            request_id.clone(), tool_name, input.clone(), None,
                                        );
                                        yield (Some(action_msg), None);
                                        current_text_message_id = uuid::Uuid::new_v4().to_string();

                                        let confirmation = rx.await.unwrap_or(PermissionConfirmation {
                                            principal_type: PrincipalType::Tool,
                                            permission: Permission::Cancel,
                                        });
                                        pending_confirmations.lock().await.remove(&request_id);

                                        let perm_resp = match confirmation.permission {
                                            Permission::AlwaysAllow | Permission::AllowOnce => {
                                                PermissionResponse::Allow {
                                                    updated_input: input,
                                                    tool_use_id,
                                                }
                                            }
                                            _ => PermissionResponse::Deny {
                                                message: "User denied the tool call".to_string(),
                                            },
                                        };
                                        let resp = ControlResponse::success(request_id, perm_resp);
                                        let mut resp_str = serde_json::to_string(&resp).map_err(|e| {
                                            ProviderError::RequestFailed(format!("Failed to serialize permission response: {e}"))
                                        })?;
                                        tracing::debug!(json = %resp_str, "can_use_tool control_response sent");
                                        resp_str.push('\n');
                                        process.stdin.write_all(resp_str.as_bytes()).await.map_err(|e| {
                                            ProviderError::RequestFailed(format!("Failed to write permission response: {e}"))
                                        })?;
                                    }
                                }
                                Some("system") if process.log_model_update => {
                                    if let Some(resolved) = parsed.get("model").and_then(|m| m.as_str()) {
                                        tracing::debug!(
                                            from = %process.current_model,
                                            to = %resolved,
                                            "set_model resolved"
                                        );
                                    }
                                    process.log_model_update = false;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        process.needs_drain = false;
                        stream_error = Some(ProviderError::RequestFailed(format!(
                            "Failed to read streaming output: {e}"
                        )));
                        break;
                    }
                }
            }

            if let Some(err) = stream_error {
                Err(err)?;
            }

            let provider_usage = ProviderUsage::new(model_name, accumulated_usage);
            yield (None, Some(provider_usage));
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::extension::Envs;
    use chrono::Utc;
    use goose_test_support::session::TEST_SESSION_ID;
    use serde_json::json;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::tempdir;
    use test_case::test_case;

    #[test_case(
        json!({"input_tokens": 100, "output_tokens": 50}),
        Some(100), Some(50)
        ; "both_tokens"
    )]
    #[test_case(json!({"input_tokens": 100}), Some(100), None ; "input_only")]
    #[test_case(json!({}), None, None ; "empty_usage")]
    fn test_extract_usage_tokens(
        usage_json: Value,
        expected_input: Option<i32>,
        expected_output: Option<i32>,
    ) {
        let usage = extract_usage_tokens(&usage_json);
        assert_eq!(usage.input_tokens, expected_input);
        assert_eq!(usage.output_tokens, expected_output);
    }

    #[tokio::test]
    async fn test_result_without_cache_fields_keeps_streamed_cache_coherent() {
        use futures::StreamExt;

        let (_provider, mut stream, _stdin_reader) = stream_with_canned_stdout(&[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":7,"cache_read_input_tokens":5000,"cache_creation_input_tokens":1000,"output_tokens":0}}}}"#,
            r#"{"type":"result","result":"Done","usage":{"input_tokens":10,"output_tokens":50}}"#,
        ])
        .await;

        let mut final_usage = None;
        while let Some(item) = stream.next().await {
            if let Ok((_, Some(usage))) = item {
                final_usage = Some(usage);
            }
        }

        let usage = final_usage.expect("stream should yield usage").usage;
        assert_eq!(usage.cache_read_input_tokens, Some(5000));
        assert_eq!(usage.cache_write_input_tokens, Some(1000));
        assert_eq!(usage.input_tokens, Some(6010)); // 10 + 5000 + 1000
        assert_eq!(usage.output_tokens, Some(50));
    }

    #[test_case(
        r#"{"type":"error","error":"context window exceeded"}"#,
        true
        ; "context_exceeded"
    )]
    #[test_case(
        r#"{"type":"error","error":"Model not supported"}"#,
        false
        ; "generic_error_from_event"
    )]
    #[test_case(r#"{"type":"error"}"#, false ; "missing_error_field")]
    fn test_error_from_event(line: &str, is_context_exceeded: bool) {
        let parsed: Value = serde_json::from_str(line).unwrap();
        let err = error_from_event("Claude CLI", &parsed);
        if is_context_exceeded {
            assert!(matches!(err, ProviderError::ContextLengthExceeded(_)));
        } else {
            assert!(matches!(err, ProviderError::RequestFailed(_)));
        }
    }

    /// (role, text, optional (image_data, mime_type))
    type MsgSpec<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);

    fn build_messages(specs: &[MsgSpec]) -> Vec<Message> {
        specs
            .iter()
            .map(|(role, text, image)| {
                let role = if *role == "user" {
                    Role::User
                } else {
                    Role::Assistant
                };
                let mut msg = Message::new(role, 0, vec![]);
                if !text.is_empty() {
                    msg = Message::new(msg.role.clone(), 0, vec![MessageContent::text(*text)]);
                }
                if let Some((data, mime)) = image {
                    msg.content.push(MessageContent::image(*data, *mime));
                }
                msg
            })
            .collect()
    }

    #[test_case(
        build_messages(&[]),
        &[]
        ; "empty"
    )]
    #[test_case(
        build_messages(&[("user", "Hello", None)]),
        &[json!({"type":"text","text":"Human: Hello"})]
        ; "single_user"
    )]
    #[test_case(
        build_messages(&[("user", "Hello", None), ("assistant", "Hi there!", None)]),
        &[json!({"type":"text","text":"Human: Hello"})]
        ; "picks_last_user_ignores_assistant"
    )]
    #[test_case(
        build_messages(&[("user", "First", None), ("assistant", "Reply", None), ("user", "Second", None)]),
        &[json!({"type":"text","text":"Human: Second"})]
        ; "multi_turn_picks_last_user"
    )]
    #[test_case(
        build_messages(&[("user", "Describe this", Some(("base64data", "image/png")))]),
        &[json!({"type":"text","text":"Human: Describe this"}),
          json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"base64data"}})]
        ; "user_with_image"
    )]
    #[test_case(
        build_messages(&[("user", "", Some(("iVBORw0KGgo", "image/png")))]),
        &[json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo"}})]
        ; "image_only"
    )]
    #[test_case(
        vec![Message::new(Role::Assistant, 0, vec![
            MessageContent::tool_request("call_123", Ok(rmcp::model::CallToolRequestParams::new("developer__shell").with_arguments(serde_json::from_value(json!({"cmd": "ls"})).unwrap())))
        ])],
        &[json!({"type":"text","text":"Assistant: [tool_use: developer__shell id=call_123]"})]
        ; "tool_request_no_user_fallback"
    )]
    #[test_case(
        vec![Message::new(Role::User, 0, vec![
            MessageContent::tool_response("call_123", Ok(rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text("file1.txt\nfile2.txt")])))
        ])],
        &[json!({"type":"text","text":"Human: [tool_result id=call_123] file1.txt\nfile2.txt"})]
        ; "tool_response"
    )]
    #[test_case(
        vec![Message::new(Role::User, 0, vec![MessageContent::text("hidden input")]).user_only()],
        &[json!({"type":"text","text":"Human: hidden input"})]
        ; "user_only_message_not_dropped"
    )]
    fn test_last_user_content_blocks(messages: Vec<Message>, expected: &[Value]) {
        let provider = make_provider();
        let blocks = provider.last_user_content_blocks(&messages);
        assert_eq!(blocks, expected);
    }

    #[test_case(
        &[json!({"type":"text","text":"Hello"})],
        json!({"type":"user","session_id":TEST_SESSION_ID,"message":{"role":"user","content":[{"type":"text","text":"Hello"}]}})
        ; "text_block"
    )]
    #[test_case(
        &[json!({"type":"text","text":"Look"}), json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}})],
        json!({"type":"user","session_id":TEST_SESSION_ID,"message":{"role":"user","content":[{"type":"text","text":"Look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}}]}})
        ; "text_and_image_blocks"
    )]
    fn test_build_stream_json_input(blocks: &[Value], expected: Value) {
        let line = build_stream_json_input(blocks, TEST_SESSION_ID);
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed, expected);
    }

    #[test_case(
        Some(json!({"models":[{"value":"default","displayName":"Default"},{"value":"sonnet","displayName":"Sonnet"},{"value":"haiku","displayName":"Haiku"}]})),
        vec!["default".into(), "sonnet".into(), "haiku".into()]
        ; "success"
    )]
    #[test_case(
        Some(json!({"models":[{"value":"default","displayName":"Default"},{"value":null,"displayName":"Bad"}]})),
        vec!["default".into()]
        ; "filters_null_values"
    )]
    #[test_case(
        None,
        vec![]
        ; "none_input"
    )]
    #[test_case(
        Some(json!({"other":"data"})),
        vec![]
        ; "no_models_key"
    )]
    fn test_extract_model_aliases(response: Option<Value>, expected: Vec<String>) {
        assert_eq!(extract_model_aliases(response.as_ref()), expected);
    }

    #[test_case(
        vec![],
        None
        ; "empty_extensions_returns_none"
    )]
    #[test_case(
        vec![ExtensionConfig::Stdio {
            name: "lookup".into(),
            description: String::new(),
            cmd: "node".into(),
            args: vec!["server.js".into()],
            envs: Envs::new([("API_KEY".into(), "secret".into())].into()),
            env_keys: vec![],
            timeout: None,
            cwd: None,
            bundled: Some(false),
            available_tools: vec![],
        }],
        Some(json!({ "mcpServers": {
            "lookup": {
                "type": "stdio",
                "command": "node",
                "args": ["server.js"],
                "env": { "API_KEY": "secret" }
            }
        }}))
        ; "stdio_converts_to_mcp_config_json"
    )]
    #[test_case(
        vec![ExtensionConfig::StreamableHttp {
            name: "lookup".into(),
            description: String::new(),
            uri: "http://localhost/mcp".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::from([("Authorization".into(), "Bearer token".into())]),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: Some(false),
            available_tools: vec![],
        }],
        Some(json!({ "mcpServers": {
            "lookup": {
                "type": "http",
                "url": "http://localhost/mcp",
                "headers": { "Authorization": "Bearer token" }
            }
        }}))
        ; "streamable_http_converts_to_mcp_config_json"
    )]
    #[test_case(
        vec![ExtensionConfig::StreamableHttp {
            name: "mcp_kiwi_com".into(),
            description: String::new(),
            uri: "https://mcp.kiwi.com".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::new(),
            timeout: None,
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: vec![],
        }],
        Some(json!({ "mcpServers": {
            "mcp_kiwi_com": {
                "type": "http",
                "url": "https://mcp.kiwi.com"
            }
        }}))
        ; "resolved_name_used_as_key"
    )]
    fn test_claude_mcp_config_json(extensions: Vec<ExtensionConfig>, expected: Option<Value>) {
        let result = claude_mcp_config_json(&extensions)
            .map(|json| serde_json::from_str::<Value>(&json).unwrap());
        assert_eq!(result, expected);
    }

    #[test]
    fn test_write_mcp_config_file() {
        let state_dir = tempdir().unwrap();
        let json = r#"{"mcpServers":{}}"#;

        let tmp = write_mcp_config_file(state_dir.path(), json).unwrap();

        assert_eq!(fs::read_to_string(tmp.path()).unwrap(), json);

        let norm_path = tmp.path().to_string_lossy().replace('\\', "/");
        let expected_prefix = format!("claude-code/mcp-config-{}_", Utc::now().format("%Y%m%d"));
        assert!(norm_path.contains(&expected_prefix));
        assert!(norm_path.ends_with(".json"));
    }

    #[test]
    fn test_write_mcp_config_file_invalid_state_dir() {
        assert!(write_mcp_config_file(Path::new("/dev/null"), "{}").is_err());
    }

    #[test]
    fn system_prompt_is_passed_by_file() {
        let state_dir = tempdir().unwrap();
        let sensitive_prompt = "private system prompt contents";
        let mut command = tokio::process::Command::new("claude");

        let system_prompt_file = ClaudeCodeProvider::apply_system_prompt_file(
            &mut command,
            state_dir.path(),
            sensitive_prompt,
        )
        .unwrap();

        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let file_arg = args
            .iter()
            .position(|arg| arg == "--system-prompt-file")
            .and_then(|index| args.get(index + 1))
            .expect("system prompt file argument should include a path");
        assert_eq!(Path::new(file_arg), system_prompt_file.path());
        assert!(!args.iter().any(|arg| arg == "--system-prompt"));
        assert!(!args.iter().any(|arg| arg.contains(sensitive_prompt)));
        assert_eq!(
            fs::read_to_string(system_prompt_file.path()).unwrap(),
            sensitive_prompt
        );
    }

    #[cfg(unix)]
    #[test]
    fn system_prompt_files_are_unpredictable_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let state_dir = tempdir().unwrap();
        let first = write_system_prompt_file(state_dir.path(), "first").unwrap();
        let second = write_system_prompt_file(state_dir.path(), "second").unwrap();

        assert_ne!(first.path(), second.path());
        assert_eq!(
            fs::metadata(first.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(second.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fn make_provider() -> ClaudeCodeProvider {
        ClaudeCodeProvider {
            command: PathBuf::from("claude"),
            name: "claude-code".to_string(),
            mcp_config_file: None,
            cli_process: tokio::sync::OnceCell::new(),
            pending_confirmations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            initial_mode: tokio::sync::Mutex::new(None),
        }
    }

    fn make_test_process_with_system_prompt(
        canned_stdout: &str,
        system_prompt_file: NamedTempFile,
    ) -> (CliProcess, tokio::io::DuplexStream) {
        let child = tokio::process::Command::new("true")
            .spawn()
            .expect("failed to spawn `true`");
        let (stdin_writer, stdin_reader) = tokio::io::duplex(1024);
        let process = CliProcess {
            child,
            stdin: Box::new(stdin_writer),
            reader: BufReader::new(Box::new(std::io::Cursor::new(
                canned_stdout.as_bytes().to_vec(),
            ))),
            stderr_handle: tokio::spawn(async { String::new() }),
            current_model: String::new(),
            log_model_update: false,
            next_request_id: 0,
            needs_drain: false,
            _system_prompt_file: system_prompt_file,
        };
        (process, stdin_reader)
    }

    fn make_test_process(canned_stdout: &str) -> (CliProcess, tokio::io::DuplexStream) {
        make_test_process_with_system_prompt(canned_stdout, NamedTempFile::new().unwrap())
    }

    #[tokio::test]
    async fn provider_reuses_system_prompt_file_and_cleans_it_on_drop() {
        let state_dir = tempdir().unwrap();
        let system_prompt_file = write_system_prompt_file(state_dir.path(), "private").unwrap();
        let system_prompt_path = system_prompt_file.path().to_path_buf();
        let (process, _stdin_reader) = make_test_process_with_system_prompt("", system_prompt_file);
        let provider = make_provider();
        provider
            .cli_process
            .set(Arc::new(tokio::sync::Mutex::new(process)))
            .unwrap();

        for _ in 0..2 {
            let process = provider.cli_process.get().unwrap().lock().await;
            assert_eq!(process._system_prompt_file.path(), system_prompt_path);
            assert!(system_prompt_path.exists());
        }

        drop(provider);
        assert!(!system_prompt_path.exists());
    }

    async fn stream_with_canned_stdout(
        canned_lines: &[&str],
    ) -> (ClaudeCodeProvider, MessageStream, tokio::io::DuplexStream) {
        let canned_stdout = canned_lines.join("\n");
        let (process, stdin_reader) = make_test_process(&canned_stdout);
        let provider = make_provider();
        let process_arc = Arc::new(tokio::sync::Mutex::new(process));
        provider.cli_process.set(process_arc).unwrap();

        let messages = vec![Message::user().with_text("test")];
        let model = ModelConfig::new(CLAUDE_CODE_DEFAULT_MODEL)
            .with_canonical_limits(CLAUDE_CODE_PROVIDER_NAME);
        let stream = provider.stream(&model, "", &messages, &[]).await.unwrap();
        (provider, stream, stdin_reader)
    }

    #[tokio::test]
    async fn former_session_title_phrase_reaches_cli() {
        use futures::StreamExt;

        let canned_stdout = [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"provider response"}}}"#,
            r#"{"type":"result","result":"provider response","usage":{}}"#,
        ]
        .join("\n");
        let (process, _stdin_reader) = make_test_process(&canned_stdout);
        let provider = make_provider();
        provider
            .cli_process
            .set(Arc::new(tokio::sync::Mutex::new(process)))
            .unwrap();
        let model = ModelConfig::new(CLAUDE_CODE_DEFAULT_MODEL)
            .with_canonical_limits(CLAUDE_CODE_PROVIDER_NAME);
        let mut stream = provider
            .stream(
                &model,
                "answer in four words or less",
                &[Message::user().with_text("ordinary request")],
                &[],
            )
            .await
            .unwrap();
        let mut response = String::new();
        while let Some(item) = stream.next().await {
            if let Some(message) = item.unwrap().0 {
                response.push_str(&message.as_concat_text());
            }
        }

        assert_eq!(response, "provider response");
    }

    async fn capture_stdin(
        provider: &ClaudeCodeProvider,
        mut reader: tokio::io::DuplexStream,
    ) -> String {
        use tokio::io::AsyncReadExt;
        provider.cli_process.get().unwrap().lock().await.stdin = Box::new(tokio::io::sink());
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn extract_permission_response(stdin_str: &str, request_id: &str) -> Value {
        let line = stdin_str
            .lines()
            .find(|l| l.contains(request_id) && l.contains("control_response"))
            .unwrap();
        let json: Value = serde_json::from_str(line).unwrap();
        json.pointer("/response/response").unwrap().clone()
    }

    #[test_case(
        &[r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#],
        Some("default"), "sonnet",
        Ok(()),
        "{\"type\":\"control_request\",\"request_id\":\"req_0\",\"request\":{\"subtype\":\"set_model\",\"model\":\"sonnet\"}}\n"
        ; "default_to_sonnet"
    )]
    #[test_case(
        &[r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#],
        Some("sonnet"), "default",
        Ok(()),
        "{\"type\":\"control_request\",\"request_id\":\"req_0\",\"request\":{\"subtype\":\"set_model\",\"model\":\"default\"}}\n"
        ; "sonnet_to_default"
    )]
    #[test_case(
        &[r#"{"type":"control_response","response":{"subtype":"error","request_id":"req_0","error":"bad model"}}"#],
        None, "bad",
        Err(ProviderError::RequestFailed("set_model failed: bad model".into())),
        "{\"type\":\"control_request\",\"request_id\":\"req_0\",\"request\":{\"subtype\":\"set_model\",\"model\":\"bad\"}}\n"
        ; "failure"
    )]
    #[test_case(
        &[],
        Some("sonnet"), "sonnet",
        Ok(()), ""
        ; "skip_when_same_model"
    )]
    #[test_case(
        &[],
        None, "sonnet",
        Err(ProviderError::RequestFailed("CLI process terminated while waiting for set_model response".into())),
        "{\"type\":\"control_request\",\"request_id\":\"req_0\",\"request\":{\"subtype\":\"set_model\",\"model\":\"sonnet\"}}\n"
        ; "eof"
    )]
    #[tokio::test]
    async fn test_send_set_model(
        lines: &[&str],
        initial_model: Option<&str>,
        target_model: &str,
        expected: Result<(), ProviderError>,
        expected_stdin: &str,
    ) {
        use tokio::io::AsyncReadExt;

        let stdout = lines.join("\n");
        let (mut process, mut stdin_reader) = make_test_process(&stdout);
        if let Some(m) = initial_model {
            process.current_model = m.to_string();
        }

        let result = process.send_set_model(target_model).await;
        process.stdin = Box::new(tokio::io::sink());
        let mut stdin_bytes = Vec::new();
        stdin_reader.read_to_end(&mut stdin_bytes).await.unwrap();

        assert_eq!(result, expected);
        if expected.is_ok() {
            assert_eq!(process.current_model, target_model);
        }
        assert_eq!(String::from_utf8(stdin_bytes).unwrap(), expected_stdin);
    }

    #[test_case(
        Permission::AllowOnce,
        json!({"behavior":"allow","updatedInput":{"path":"foo.txt","content":"hello"},"toolUseID":"tu_1"})
        ; "allow"
    )]
    #[test_case(
        Permission::DenyOnce,
        json!({"behavior":"deny","message":"User denied the tool call"})
        ; "deny"
    )]
    #[tokio::test]
    async fn test_can_use_tool(permission: Permission, expected_response: Value) {
        use futures::StreamExt;

        let (provider, mut stream, stdin_reader) = stream_with_canned_stdout(&[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"control_request","request_id":"perm_1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"path":"foo.txt","content":"hello"},"tool_use_id":"tu_1"}}"#,
            r#"{"type":"result","result":"Done","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]).await;

        let (first_msg, _) = stream.next().await.unwrap().unwrap();
        let first_msg = first_msg.unwrap();
        let ar = first_msg
            .content
            .iter()
            .find_map(|c| c.as_action_required())
            .unwrap();
        match &ar.data {
            crate::conversation::message::ActionRequiredData::ToolConfirmation {
                id,
                tool_name,
                ..
            } => {
                assert_eq!(id, "perm_1");
                assert_eq!(tool_name, "Write");
            }
            _ => panic!("expected ToolConfirmation"),
        }

        let handled = provider
            .handle_permission_confirmation(
                "perm_1",
                &PermissionConfirmation {
                    principal_type: PrincipalType::Tool,
                    permission: permission.clone(),
                },
            )
            .await;
        assert!(handled);
        assert!(provider.pending_confirmations.lock().await.is_empty());

        while let Some(item) = stream.next().await {
            item.unwrap();
        }
        drop(stream);

        let stdin_str = capture_stdin(&provider, stdin_reader).await;
        let response_data = extract_permission_response(&stdin_str, "perm_1");
        assert_eq!(response_data, expected_response);
    }

    #[tokio::test]
    async fn test_text_message_id_rotates_after_action_required() {
        use futures::StreamExt;

        let (provider, mut stream, _stdin_reader) = stream_with_canned_stdout(&[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"before permission"}}}"#,
            r#"{"type":"control_request","request_id":"perm_1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"path":"foo.txt","content":"hello"},"tool_use_id":"tu_1"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"after permission"}}}"#,
            r#"{"type":"result","result":"Done","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]).await;

        let (before_msg, usage) = stream.next().await.unwrap().unwrap();
        assert!(usage.is_none());
        let before_msg = before_msg.unwrap();
        assert_eq!(before_msg.role, Role::Assistant);
        assert_eq!(before_msg.as_concat_text(), "before permission");
        let before_id = before_msg
            .id
            .as_deref()
            .expect("text before permission should have a provider ID")
            .to_string();

        let (action_msg, usage) = stream.next().await.unwrap().unwrap();
        assert!(usage.is_none());
        assert!(action_msg
            .unwrap()
            .content
            .iter()
            .any(|content| content.as_action_required().is_some()));

        let handled = provider
            .handle_permission_confirmation(
                "perm_1",
                &PermissionConfirmation {
                    principal_type: PrincipalType::Tool,
                    permission: Permission::AllowOnce,
                },
            )
            .await;
        assert!(handled);

        let (after_msg, usage) = stream.next().await.unwrap().unwrap();
        assert!(usage.is_none());
        let after_msg = after_msg.unwrap();
        assert_eq!(after_msg.role, Role::Assistant);
        assert_eq!(after_msg.as_concat_text(), "after permission");
        let after_id = after_msg
            .id
            .as_deref()
            .expect("text after permission should have a provider ID");

        assert_ne!(before_id, after_id);

        while let Some(item) = stream.next().await {
            item.unwrap();
        }
    }

    #[tokio::test]
    async fn test_can_use_tool_cancel_on_drop() {
        use futures::StreamExt;

        let (provider, mut stream, stdin_reader) = stream_with_canned_stdout(&[
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"control_request","request_id":"perm_1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"path":"foo.txt"},"tool_use_id":"tu_1"}}"#,
            r#"{"type":"result","result":"Done","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]).await;

        let pending = Arc::clone(&provider.pending_confirmations);

        let (first_msg, _) = stream.next().await.unwrap().unwrap();
        assert!(first_msg
            .unwrap()
            .content
            .iter()
            .any(|c| c.as_action_required().is_some()));

        let tx = pending.lock().await.remove("perm_1").unwrap();
        drop(tx);

        while let Some(item) = stream.next().await {
            item.unwrap();
        }
        drop(stream);

        let stdin_str = capture_stdin(&provider, stdin_reader).await;
        let response_data = extract_permission_response(&stdin_str, "perm_1");
        assert_eq!(
            response_data,
            json!({"behavior":"deny","message":"User denied the tool call"})
        );
    }

    #[tokio::test]
    async fn test_pending_permissions_cleaned_on_new_stream() {
        use futures::StreamExt;

        let canned_stdout = [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_0"}}"#,
            r#"{"type":"result","result":"Done","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ]
        .join("\n");

        let (process, stdin_reader) = make_test_process(&canned_stdout);
        let provider = make_provider();
        let process_arc = Arc::new(tokio::sync::Mutex::new(process));
        provider.cli_process.set(process_arc).unwrap();

        let (tx, _rx) = oneshot::channel();
        provider
            .pending_confirmations
            .lock()
            .await
            .insert("stale_1".to_string(), tx);

        let messages = vec![Message::user().with_text("test")];
        let model = ModelConfig::new(CLAUDE_CODE_DEFAULT_MODEL)
            .with_canonical_limits(CLAUDE_CODE_PROVIDER_NAME);
        let mut stream = provider.stream(&model, "", &messages, &[]).await.unwrap();

        while let Some(item) = stream.next().await {
            item.unwrap();
        }
        drop(stream);

        assert!(provider.pending_confirmations.lock().await.is_empty());

        let stdin_str = capture_stdin(&provider, stdin_reader).await;
        let response_data = extract_permission_response(&stdin_str, "stale_1");
        assert_eq!(response_data["behavior"], "deny");
    }
}
