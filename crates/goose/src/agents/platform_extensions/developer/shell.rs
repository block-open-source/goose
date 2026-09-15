#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(windows))]
use std::sync::Arc;
#[cfg(not(windows))]
use std::sync::Mutex;
use std::time::Duration;

use rmcp::model::{Annotations, CallToolResult, ContentBlock, TextContent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
#[cfg(not(windows))]
use tokio::sync::OnceCell;
#[cfg(not(windows))]
use tokio::task::JoinHandle;
use tokio_stream::{wrappers::SplitStream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::agents::tool_execution::ToolCallNotificationEmitter;
use crate::subprocess::SubprocessExt;

pub use super::shell_output_streaming::{
    parse_shell_output_notification, ShellOutputNotificationChunk, ShellOutputNotificationParams,
    ShellOutputStream, DEVELOPER_SHELL_OUTPUT_NOTIFICATION_METHOD,
};
use super::shell_output_streaming::{ShellOutputBatcher, SHELL_LIVE_OUTPUT_FLUSH_INTERVAL};

/// Check if the current process is running inside a Flatpak sandbox.
///
/// When inside Flatpak, shell commands must be wrapped with `flatpak-spawn --host`
/// to execute on the host system rather than inside the sandbox.
#[cfg(not(windows))]
pub(crate) fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

#[cfg(not(windows))]
const FLATPAK_HOST_ARGS: [&str; 2] = ["--host", "--watch-bus"];

#[cfg(not(windows))]
pub(crate) fn flatpak_spawn_command() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("flatpak-spawn");
    command.args(FLATPAK_HOST_ARGS);
    command
}

#[cfg(not(windows))]
fn flatpak_spawn_process() -> std::process::Command {
    let mut command = std::process::Command::new("flatpak-spawn");
    command.args(FLATPAK_HOST_ARGS);
    command
}

#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnixShellFlavor {
    Posix,
    Nushell,
}

#[cfg(not(windows))]
fn unix_shell_flavor(shell: &str) -> UnixShellFlavor {
    let name = std::path::Path::new(shell)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase();

    match name.as_str() {
        "nu" | "nushell" => UnixShellFlavor::Nushell,
        _ => UnixShellFlavor::Posix,
    }
}

#[cfg(not(windows))]
fn unix_login_shell_command_args(shell: &str) -> [&'static str; 4] {
    let probe = match unix_shell_flavor(shell) {
        UnixShellFlavor::Nushell => "print ($env.PATH | str join (char esep))",
        UnixShellFlavor::Posix => "echo $PATH",
    };

    ["-l", "-i", "-c", probe]
}

#[cfg(not(windows))]
fn unix_shell_command_args(command_line: &str) -> [&str; 2] {
    ["-c", command_line]
}

/// Resolve the shell used to run Developer extension commands on Windows,
/// respecting `GOOSE_SHELL`.
///
/// Defaults to `cmd` when `GOOSE_SHELL` is unset. The invocation flags are
/// chosen automatically from the executable name in `build_shell_command`,
/// so callers only ever provide a bare executable path or name — see that
/// function for the flag mapping.
#[cfg(windows)]
fn windows_shell() -> String {
    std::env::var("GOOSE_SHELL").unwrap_or_else(|_| "cmd".to_string())
}

/// Short, human-readable name of a shell path (the file stem), used both to
/// pick the right argument style on Windows and to tell the LLM which
/// dialect to write in the tool description.
#[cfg(windows)]
fn shell_basename(shell: &str) -> String {
    Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cmd")
        .to_lowercase()
}

#[cfg(not(windows))]
fn shell_basename(shell: &str) -> String {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_string()
}

/// Basename of the shell the `shell` tool will invoke, for use in the tool
/// description so the LLM knows which dialect to write.
#[cfg(windows)]
pub fn shell_display_name() -> String {
    shell_basename(&windows_shell())
}

#[cfg(not(windows))]
pub fn shell_display_name() -> String {
    shell_basename(&unix_shell())
}

/// The shell tool runs commands with `-c "..."`, and LLMs routinely emit
/// POSIX-style patterns such as heredocs (`cat <<EOF > file`), `$VAR`
/// expansion, and `2>&1` redirection. Non-POSIX shells (fish, csh, tcsh,
/// nu, ...) reject or mis-interpret these, so we don't auto-select based
/// on `$SHELL`: we check whether `bash` is on PATH and otherwise fall back
/// to `sh`. Users who really want their login shell can opt in via
/// `GOOSE_SHELL`.
#[cfg(not(windows))]
fn unix_shell() -> String {
    if let Ok(shell) = std::env::var("GOOSE_SHELL") {
        return shell;
    }
    if which::which("bash").is_ok() {
        "bash".to_string()
    } else {
        "sh".to_string()
    }
}

const OUTPUT_LIMIT_LINES: usize = 2000;
pub const OUTPUT_LIMIT_BYTES: usize = 50_000;
const OUTPUT_PREVIEW_LINES: usize = 50;
const OUTPUT_PREVIEW_BYTES: usize = 10_000;

const OUTPUT_SLOTS: usize = 8;

/// Result of truncating command output.
struct TruncateResult {
    /// The (possibly truncated) text to display.
    text: String,
    /// When output was truncated, the path where the full output was saved
    /// and a human-readable reason for the truncation.
    truncation: Option<TruncationInfo>,
}

struct TruncationInfo {
    path: PathBuf,
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShellParams {
    pub command: String,
    /// Maximum time in seconds to allow the command to run before it is killed.
    /// If omitted, defaults to DEFAULT_EXTENSION_TIMEOUT.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    /// Process exit code. 0 indicates success, non-zero indicates failure.
    /// Absent if the process was killed (e.g. timeout).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// True if the command was killed because it exceeded the timeout.
    #[serde(default)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// True if output collection was cut short after the shell exited.
    #[serde(default)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub output_truncated: bool,
    /// Error reported by output collection after process exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_collection_error: Option<String>,
}

/// Resolve the user's full PATH by running a login shell.
///
/// When goosed is launched from a desktop app (e.g. Electron), it may inherit
/// a minimal PATH like `/usr/bin:/bin`. This function spawns a login shell to
/// source the user's profile and recover the full PATH.
#[cfg(not(windows))]
pub(crate) fn resolve_login_shell_path() -> Option<String> {
    use process_wrap::std::{CommandWrap, ProcessSession};

    let shell = unix_shell();
    let login_args = unix_login_shell_command_args(&shell);

    // Build the command, varying only the flatpak vs direct invocation.
    let mut cmd = if is_flatpak() {
        let mut c = flatpak_spawn_process();
        c.arg(&shell).args(login_args);
        CommandWrap::from(c)
    } else {
        let mut c = std::process::Command::new(&shell);
        c.args(login_args);
        CommandWrap::from(c)
    };

    cmd.command_mut()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // Spawn in a new session so that bash's interactive job-control setup
    // (TIOCSPGRP) cannot steal the terminal foreground from goose, which
    // would cause goose to receive SIGTTIN and be suspended on startup.
    cmd.wrap(ProcessSession);

    let mut child = cmd.spawn().ok()?;

    let mut stdout = child.stdout().take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        use std::io::Read;
        if stdout.read_to_end(&mut buf).is_ok() {
            let _ = tx.send(buf);
        }
    });

    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(buf)
            if child
                .wait()
                .is_ok_and(|s: std::process::ExitStatus| s.success()) =>
        {
            // Take the last non-empty line — interactive shells may emit
            // extra output from profile scripts before our echo.
            String::from_utf8_lossy(&buf)
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
                .filter(|path| !path.is_empty())
        }
        _ => {
            let _ = child.kill();
            None
        }
    }
}

/// Resolves the user's login-shell PATH in the background.
///
/// Spawned at `ShellTool` construction so the ~hundreds-of-ms cost of sourcing
/// the user's shell profile overlaps with the rest of agent setup and the
/// first LLM turn. The first `shell` invocation awaits the result; subsequent
/// invocations read from the cached cell.
#[cfg(not(windows))]
struct LoginPath {
    cell: OnceCell<Option<Arc<str>>>,
    handle: Mutex<Option<JoinHandle<Option<String>>>>,
}

#[cfg(not(windows))]
impl LoginPath {
    fn spawn() -> Self {
        let handle = tokio::task::spawn_blocking(resolve_login_shell_path);
        Self {
            cell: OnceCell::new(),
            handle: Mutex::new(Some(handle)),
        }
    }

    fn resolved(value: Option<String>) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(value.map(Arc::from));
        Self {
            cell,
            handle: Mutex::new(None),
        }
    }

    async fn get(&self) -> Option<Arc<str>> {
        self.cell
            .get_or_init(|| async {
                let handle = self
                    .handle
                    .lock()
                    .expect("login_path mutex poisoned")
                    .take();
                match handle {
                    Some(h) => h.await.ok().flatten().map(Arc::from),
                    None => None,
                }
            })
            .await
            .clone()
    }
}

pub struct ShellTool {
    output_dir: tempfile::TempDir,
    call_index: AtomicUsize,
    #[cfg(not(windows))]
    login_path: LoginPath,
}

impl ShellTool {
    pub fn new(use_login_shell_path: bool) -> std::io::Result<Self> {
        #[cfg(windows)]
        let _unused = use_login_shell_path;
        Ok(Self {
            output_dir: tempfile::tempdir()?,
            call_index: AtomicUsize::new(0),
            #[cfg(not(windows))]
            login_path: if use_login_shell_path {
                LoginPath::spawn()
            } else {
                LoginPath::resolved(None)
            },
        })
    }

    #[cfg(test)]
    pub fn new_for_test() -> std::io::Result<Self> {
        Ok(Self {
            output_dir: tempfile::tempdir()?,
            call_index: AtomicUsize::new(0),
            #[cfg(not(windows))]
            login_path: LoginPath::resolved(None),
        })
    }

    fn visible_text(text: impl Into<String>) -> ContentBlock {
        ContentBlock::Text(
            TextContent::new(text).with_annotations(Annotations::default().with_priority(0.0)),
        )
    }

    pub async fn shell(&self, params: ShellParams) -> CallToolResult {
        self.shell_with_cwd(params, None, None, CancellationToken::new())
            .await
    }

    pub async fn shell_with_cwd(
        &self,
        params: ShellParams,
        working_dir: Option<&std::path::Path>,
        session_id: Option<&str>,
        cancellation_token: CancellationToken,
    ) -> CallToolResult {
        self.shell_with_cwd_and_emitter(
            params,
            working_dir,
            session_id,
            None,
            None,
            cancellation_token,
        )
        .await
    }

    pub(crate) async fn shell_with_cwd_and_emitter(
        &self,
        params: ShellParams,
        working_dir: Option<&std::path::Path>,
        session_id: Option<&str>,
        model_name: Option<&str>,
        notification_emitter: Option<ToolCallNotificationEmitter>,
        cancellation_token: CancellationToken,
    ) -> CallToolResult {
        if params.command.trim().is_empty() {
            return Self::error_result("Command cannot be empty.", None);
        }

        #[cfg(windows)]
        if shell_basename(&windows_shell()) == "cmd" && params.command.contains(['\n', '\r']) {
            return Self::error_result(
                "cmd.exe silently truncates commands at newlines — only the first line executes, \
                 with exit code 0. Use `&` to chain commands on one line \
                 (e.g. `echo a & echo b`), or set GOOSE_SHELL=powershell.",
                None,
            );
        }

        #[cfg(not(windows))]
        let login_path = self.login_path.get().await;
        #[cfg(not(windows))]
        let login_path_ref = login_path.as_deref();
        #[cfg(windows)]
        let login_path_ref: Option<&str> = None;

        let execution = match run_command(
            &params.command,
            params.timeout_secs,
            working_dir,
            login_path_ref,
            session_id,
            model_name,
            notification_emitter,
            cancellation_token,
        )
        .await
        {
            Ok(execution) => execution,
            Err(error) => return Self::error_result(&error, None),
        };

        // Derive stdout, stderr, and interleaved display from the single tagged-line buffer
        let (raw_stdout, raw_stderr, interleaved) = split_lines(&execution.lines);

        let output_dir = self.output_dir.path();
        let slot = self.call_index.fetch_add(1, Ordering::Relaxed) % OUTPUT_SLOTS;
        let stdout_result = if raw_stdout.is_empty() {
            TruncateResult {
                text: String::new(),
                truncation: None,
            }
        } else {
            match truncate_output(&raw_stdout, &format!("stdout-{slot}"), output_dir) {
                Ok(r) => r,
                Err(error) => return Self::error_result(&error, None),
            }
        };
        let stderr_result = if raw_stderr.is_empty() {
            TruncateResult {
                text: String::new(),
                truncation: None,
            }
        } else {
            match truncate_output(&raw_stderr, &format!("stderr-{slot}"), output_dir) {
                Ok(r) => r,
                Err(error) => return Self::error_result(&error, None),
            }
        };

        let shell_output = ShellOutput {
            stdout: stdout_result.text,
            stderr: stderr_result.text,
            exit_code: execution.exit_code,
            timed_out: execution.timed_out,
            output_truncated: execution.output_truncated,
            output_collection_error: execution.output_collection_error.clone(),
        };
        let structured_content = serde_json::to_value(&shell_output).ok();
        let render_result = match render_output(&interleaved, &format!("output-{slot}"), output_dir)
        {
            Ok(r) => r,
            Err(error) => return Self::error_result(&error, None),
        };
        let mut rendered = render_result.text;

        // Collect truncation notices from stdout, stderr, and interleaved output.
        // These are delivered as a separate Content block so the model sees them as
        // instructions rather than part of the command's data output.
        let truncation_notices: Vec<String> = [
            &stdout_result.truncation,
            &stderr_result.truncation,
            &render_result.truncation,
        ]
        .iter()
        .filter_map(|t| t.as_ref().map(truncation_notice))
        .collect();

        let is_error = if execution.timed_out {
            rendered.push_str(&format!(
                "\n\nCommand timed out after {} seconds",
                resolve_shell_timeout(params.timeout_secs)
            ));
            true
        } else {
            execution.exit_code.unwrap_or(1) != 0
        };

        if execution.output_truncated {
            rendered.push_str(
                "\n\nOutput may be incomplete because stream draining timed out after process exit.",
            );
        }
        if let Some(error) = &execution.output_collection_error {
            rendered.push_str(&format!(
                "\n\nOutput collection error occurred; output may be incomplete: {error}"
            ));
        }

        let is_error = is_error || execution.output_collection_error.is_some();

        if is_error {
            if let Some(code) = execution.exit_code.filter(|c| *c != 0) {
                rendered.push_str(&format!("\n\nCommand exited with code {code}"));
            }
            let mut error_blocks = vec![Self::visible_text(rendered)];
            if !truncation_notices.is_empty() {
                error_blocks.push(Self::visible_text(truncation_notices.join("\n")));
            }
            let mut result = CallToolResult::error(error_blocks);
            result.structured_content = structured_content;
            return result;
        }

        let mut content_blocks = vec![Self::visible_text(rendered)];
        if !truncation_notices.is_empty() {
            content_blocks.push(Self::visible_text(truncation_notices.join("\n")));
        }
        let mut result = CallToolResult::success(content_blocks);
        result.structured_content = structured_content;
        result
    }

    pub fn error_result(message: &str, exit_code: Option<i32>) -> CallToolResult {
        let shell_output = ShellOutput {
            stdout: String::new(),
            stderr: message.to_string(),
            exit_code,
            timed_out: false,
            output_truncated: false,
            output_collection_error: None,
        };
        let mut result = CallToolResult::error(vec![Self::visible_text(message)]);
        result.structured_content = serde_json::to_value(&shell_output).ok();
        result
    }
}

struct ExecutionOutput {
    /// Lines in arrival order, tagged by source: (is_stderr, text)
    lines: Vec<(bool, String)>,
    exit_code: Option<i32>,
    timed_out: bool,
    output_truncated: bool,
    output_collection_error: Option<String>,
}

fn resolve_shell_timeout(timeout_secs: Option<u64>) -> u64 {
    timeout_secs.unwrap_or_else(|| {
        crate::config::Config::global()
            .get_goose_default_extension_timeout()
            .unwrap_or(crate::config::DEFAULT_EXTENSION_TIMEOUT)
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    command_line: &str,
    timeout_secs: Option<u64>,
    working_dir: Option<&std::path::Path>,
    login_path: Option<&str>,
    session_id: Option<&str>,
    model_name: Option<&str>,
    notification_emitter: Option<ToolCallNotificationEmitter>,
    cancellation_token: CancellationToken,
) -> Result<ExecutionOutput, String> {
    let timeout_secs = Some(resolve_shell_timeout(timeout_secs));

    let mut command = build_shell_command(
        command_line,
        working_dir,
        login_path,
        session_id,
        model_name,
    );

    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to spawn shell command: {}", error))?;

    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture stdout".to_string())?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture stderr".to_string())?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let output_task = tokio::spawn(collect_tagged_lines(
        child_stdout,
        child_stderr,
        tx,
        notification_emitter,
    ));
    let abort_handle = output_task.abort_handle();

    let mut timed_out = false;
    let exit_code = if let Some(timeout_secs) = timeout_secs.filter(|value| *value > 0) {
        tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()) => match result {
                Ok(wait_result) => wait_result
                    .map_err(|error| format!("Failed waiting on shell command: {}", error))?
                    .code(),
                Err(_) => {
                    timed_out = true;
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    None
                }
            },
            _ = cancellation_token.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                None
            }
        }
    } else {
        tokio::select! {
            result = child.wait() => result
                .map_err(|error| format!("Failed waiting on shell command: {}", error))?
                .code(),
            _ = cancellation_token.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                None
            }
        }
    };

    const OUTPUT_DRAIN_TIMEOUT_MILLIS: u64 = 500;
    let mut output_collection_error = None;
    let output_truncated = match tokio::time::timeout(
        Duration::from_millis(OUTPUT_DRAIN_TIMEOUT_MILLIS),
        output_task,
    )
    .await
    {
        Ok(Ok(Ok(()))) => false,
        Ok(Ok(Err(e))) => {
            output_collection_error = Some(format!("Failed to collect shell output: {}", e));
            false
        }
        Ok(Err(e)) => {
            output_collection_error = Some(format!("Failed to collect shell output: {}", e));
            false
        }
        Err(_) => {
            tracing::debug!(
                "output drain timed out after {OUTPUT_DRAIN_TIMEOUT_MILLIS}ms (backgrounded process?)"
            );
            abort_handle.abort();
            true
        }
    };

    rx.close();
    let mut lines = Vec::new();
    while let Some(item) = rx.recv().await {
        lines.push(item);
    }

    Ok(ExecutionOutput {
        lines,
        exit_code,
        timed_out,
        output_truncated,
        output_collection_error,
    })
}

/// Build the `Command` that executes a single command line via the configured shell.
///
/// The invocation flags are selected automatically from the shell executable
/// name, so setting `GOOSE_SHELL` to a bare executable is enough — users never
/// need to supply command-style flags themselves:
///
/// - PowerShell (`pwsh`, `powershell`) → `-NoProfile -NonInteractive -Command`
/// - cmd (`cmd`) → `/C`
/// - POSIX shells (bash, zsh, … via Cygwin/MSYS2) → `-c`
///
/// On Unix the default shell (`bash`, falling back to `sh`) is likewise invoked
/// as `<shell> -c <line>`.
fn build_shell_command(
    command_line: &str,
    working_dir: Option<&std::path::Path>,
    login_path: Option<&str>,
    session_id: Option<&str>,
    model_name: Option<&str>,
) -> tokio::process::Command {
    #[cfg(windows)]
    let mut command = {
        let shell = windows_shell();
        let shell_stem = shell_basename(&shell);
        let mut command = tokio::process::Command::new(&shell);
        match shell_stem.as_str() {
            "pwsh" | "powershell" => {
                command.args(["-NoProfile", "-NonInteractive", "-Command", command_line]);
            }
            "cmd" => {
                command.arg("/C").raw_arg(command_line);
            }
            // POSIX-like shells (bash, zsh, etc.) on Windows (e.g. Cygwin/MSYS2)
            _ => {
                command.args(["-c", command_line]);
            }
        }
        if let Some(path) = working_dir {
            command.current_dir(path);
        }
        if let Some(path) = login_path {
            command.env("PATH", path);
        }
        command
    };

    #[cfg(not(windows))]
    let mut command = {
        let shell = unix_shell();

        if is_flatpak() {
            let mut command = flatpak_spawn_command();
            if let Some(dir) = working_dir {
                command.arg(format!("--directory={}", dir.display()));
            }
            if let Some(path) = login_path {
                command.arg(format!("--env=PATH={}", path));
            }
            apply_flatpak_session_environment(&mut command, session_id);
            apply_flatpak_model_environment(&mut command, model_name);
            command
                .arg(&shell)
                .args(unix_shell_command_args(command_line));
            command
        } else {
            let mut command = tokio::process::Command::new(shell);
            command.args(unix_shell_command_args(command_line));
            if let Some(path) = working_dir {
                command.current_dir(path);
            }
            if let Some(path) = login_path {
                command.env("PATH", path);
            }
            apply_session_environment(&mut command, session_id);
            apply_model_environment(&mut command, model_name);
            command
        }
    };

    #[cfg(windows)]
    {
        apply_session_environment(&mut command, session_id);
        apply_model_environment(&mut command, model_name);
    }
    command.set_no_window();
    command
}

fn apply_session_environment(command: &mut tokio::process::Command, session_id: Option<&str>) {
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        command.env("AGENT_SESSION_ID", session_id);
    } else {
        command.env_remove("AGENT_SESSION_ID");
    }
}

fn apply_model_environment(command: &mut tokio::process::Command, model_name: Option<&str>) {
    if let Some(model_name) = model_name {
        command.env("GOOSE_MODEL", model_name);
    } else {
        command.env_remove("GOOSE_MODEL");
    }
}

#[cfg(not(windows))]
fn apply_flatpak_session_environment(
    command: &mut tokio::process::Command,
    session_id: Option<&str>,
) {
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        command.arg(format!("--env=AGENT_SESSION_ID={session_id}"));
    } else {
        command.arg("--unset-env=AGENT_SESSION_ID");
    }
}

#[cfg(not(windows))]
fn apply_flatpak_model_environment(
    command: &mut tokio::process::Command,
    model_name: Option<&str>,
) {
    if let Some(model_name) = model_name {
        command.arg(format!("--env=GOOSE_MODEL={model_name}"));
    } else {
        command.arg("--unset-env=GOOSE_MODEL");
    }
}

/// Split tagged lines into (stdout, stderr, interleaved) strings.
fn split_lines(lines: &[(bool, String)]) -> (String, String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut interleaved = String::new();
    let mut stdout_started = false;
    let mut stderr_started = false;
    for (i, (is_stderr, text)) in lines.iter().enumerate() {
        if i > 0 {
            interleaved.push('\n');
        }
        interleaved.push_str(text);
        let (target, started) = if *is_stderr {
            (&mut stderr, &mut stderr_started)
        } else {
            (&mut stdout, &mut stdout_started)
        };
        if *started {
            target.push('\n');
        }
        *started = true;
        target.push_str(text);
    }
    (stdout, stderr, interleaved)
}

/// Collect lines from stdout and stderr and send `(is_stderr, line)` tuples to `tx`.
async fn collect_tagged_lines(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    tx: tokio::sync::mpsc::UnboundedSender<(bool, String)>,
    notification_emitter: Option<ToolCallNotificationEmitter>,
) -> Result<(), std::io::Error> {
    let stdout_lines = SplitStream::new(BufReader::new(stdout).split(b'\n')).map(|l| (false, l));
    let stderr_lines = SplitStream::new(BufReader::new(stderr).split(b'\n')).map(|l| (true, l));
    let mut merged = stdout_lines.merge(stderr_lines);
    let mut output_batcher = notification_emitter.map(ShellOutputBatcher::new);
    let mut flush_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + SHELL_LIVE_OUTPUT_FLUSH_INTERVAL,
        SHELL_LIVE_OUTPUT_FLUSH_INTERVAL,
    );
    flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            tagged_line = merged.next() => {
                let Some((is_stderr, line)) = tagged_line else {
                    break;
                };
                let line = match line {
                    Ok(line) => String::from_utf8_lossy(&line).into_owned(),
                    Err(error) => {
                        if let Some(output_batcher) = output_batcher.as_mut() {
                            output_batcher.flush();
                        }
                        return Err(error);
                    }
                };

                if let Some(output_batcher) = output_batcher.as_mut() {
                    if output_batcher.push_line(is_stderr, &line) {
                        flush_interval.reset();
                    }
                }
                let _ = tx.send((is_stderr, line));
            }
            _ = flush_interval.tick(), if output_batcher.is_some() => {
                if let Some(output_batcher) = output_batcher.as_mut() {
                    output_batcher.flush();
                }
            }
        }
    }

    if let Some(output_batcher) = output_batcher.as_mut() {
        output_batcher.flush();
    }
    Ok(())
}

/// Build a human-readable truncation notice with platform-appropriate commands.
fn truncation_notice(info: &TruncationInfo) -> String {
    let path = info.path.display();
    let commands = if cfg!(windows) {
        "PowerShell commands like `Get-Content -TotalCount 200`, `Select-String`, or \
         `Get-Content | Select-Object -Skip 100 -First 100`"
    } else {
        "shell commands like `head`, `tail`, or `sed -n '100,200p'`"
    };
    format!(
        "[{reason} Full output saved to {path}. \
         Read it with {commands} up to {limit} lines at a time.]",
        reason = info.reason,
        limit = OUTPUT_LIMIT_LINES,
    )
}

fn render_output(
    full_output: &str,
    label: &str,
    output_dir: &std::path::Path,
) -> Result<TruncateResult, String> {
    if full_output.is_empty() {
        return Ok(TruncateResult {
            text: "(no output)".to_string(),
            truncation: None,
        });
    }
    truncate_output(full_output, label, output_dir)
}

fn truncate_output(
    full_output: &str,
    label: &str,
    output_dir: &std::path::Path,
) -> Result<TruncateResult, String> {
    let lines: Vec<&str> = full_output.split('\n').collect();
    let total_lines = lines.len();
    let total_bytes = full_output.len();

    let exceeded_lines = total_lines > OUTPUT_LIMIT_LINES;
    let exceeded_bytes = total_bytes > OUTPUT_LIMIT_BYTES;

    if !exceeded_lines && !exceeded_bytes {
        return Ok(TruncateResult {
            text: full_output.to_string(),
            truncation: None,
        });
    }

    let output_path = save_full_output(full_output, label, output_dir)?;

    let preview_start = total_lines.saturating_sub(OUTPUT_PREVIEW_LINES);
    let preview = truncate_preview_bytes(lines[preview_start..].join("\n"));

    let reason = if exceeded_lines {
        format!("Output exceeded {OUTPUT_LIMIT_LINES} line limit ({total_lines} lines total).")
    } else {
        format!(
            "Output exceeded {} byte limit ({total_bytes} bytes total).",
            OUTPUT_LIMIT_BYTES
        )
    };

    Ok(TruncateResult {
        text: preview,
        truncation: Some(TruncationInfo {
            path: output_path,
            reason,
        }),
    })
}

/// Keep the preview's tail within OUTPUT_PREVIEW_BYTES so lines without
/// newlines (progress bars using `\r`, minified or base64 content) cannot
/// smuggle an unbounded preview past the line-based truncation.
#[allow(clippy::string_slice)] // The start index is snapped to a char boundary.
fn truncate_preview_bytes(preview: String) -> String {
    if preview.len() <= OUTPUT_PREVIEW_BYTES {
        return preview;
    }
    let mut start = preview.len() - OUTPUT_PREVIEW_BYTES;
    while !preview.is_char_boundary(start) {
        start += 1;
    }
    preview[start..].to_string()
}

fn save_full_output(
    output: &str,
    label: &str,
    output_dir: &std::path::Path,
) -> Result<PathBuf, String> {
    let path = output_dir.join(label);
    std::fs::write(&path, output).map_err(|e| format!("Failed to write output buffer: {e}"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ContentBlock;

    fn extract_text(result: &CallToolResult) -> &str {
        match &result.content[0] {
            ContentBlock::Text(text) => &text.text,
            _ => panic!("expected text"),
        }
    }

    fn extract_shell_output(result: &CallToolResult) -> ShellOutput {
        let value = result
            .structured_content
            .clone()
            .expect("expected structured content");
        serde_json::from_value(value).expect("expected shell output structured content")
    }

    #[tokio::test]
    async fn shell_executes_command() {
        let tool = ShellTool::new_for_test().unwrap();
        let result = tool
            .shell(ShellParams {
                command: "echo hello".to_string(),
                timeout_secs: None,
            })
            .await;

        assert_eq!(result.is_error, Some(false));
        assert!(extract_text(&result).contains("hello"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn full_live_notification_channel_does_not_change_final_output() {
        let tool = ShellTool::new_for_test().unwrap();
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let result = tool
            .shell_with_cwd_and_emitter(
                ShellParams {
                    command: "printf 'out-1\\nout-2\\n'; printf 'err-1\\nerr-2\\n' >&2".to_string(),
                    timeout_secs: None,
                },
                None,
                None,
                None,
                Some(ToolCallNotificationEmitter::new(sender)),
                CancellationToken::new(),
            )
            .await;

        let shell_output = extract_shell_output(&result);
        assert_eq!(result.is_error, Some(false));
        assert_eq!(shell_output.stdout, "out-1\nout-2");
        assert_eq!(shell_output.stderr, "err-1\nerr-2");
        assert_eq!(shell_output.exit_code, Some(0));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_exports_model_name() {
        let tool = ShellTool::new_for_test().unwrap();
        let expected_model = "session-model";
        let result = tool
            .shell_with_cwd_and_emitter(
                ShellParams {
                    command: "printf '%s' \"$GOOSE_MODEL\"".to_string(),
                    timeout_secs: None,
                },
                None,
                None,
                Some(expected_model),
                None,
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.is_error, Some(false));
        assert_eq!(extract_text(&result), expected_model);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_removes_inherited_model_name_when_not_provided() {
        let _env = env_lock::lock_env([("GOOSE_MODEL", Some("stale-model"))]);
        let tool = ShellTool::new_for_test().unwrap();
        let result = tool
            .shell_with_cwd_and_emitter(
                ShellParams {
                    command: "if [ -n \"$GOOSE_MODEL\" ]; then printf '%s' \"$GOOSE_MODEL\"; else printf '<unset>'; fi".to_string(),
                    timeout_secs: None,
                },
                None,
                None,
                None,
                None,
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.is_error, Some(false));
        assert_eq!(extract_text(&result), "<unset>");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_returns_error_for_non_zero_exit() {
        let tool = ShellTool::new_for_test().unwrap();
        let result = tool
            .shell(ShellParams {
                command: "echo fail && exit 7".to_string(),
                timeout_secs: None,
            })
            .await;

        assert_eq!(result.is_error, Some(true));
        assert!(extract_text(&result).contains("Command exited with code 7"));
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_uses_working_dir_for_relative_execution() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellTool::new_for_test().unwrap();
        let result = tool
            .shell_with_cwd(
                ShellParams {
                    command: "pwd".to_string(),
                    timeout_secs: None,
                },
                Some(dir.path()),
                None,
                CancellationToken::new(),
            )
            .await;

        assert_eq!(result.is_error, Some(false));
        let observed = std::fs::canonicalize(extract_text(&result)).unwrap();
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(observed, expected);
    }

    #[test]
    fn model_environment_is_set_or_removed() {
        for (model_name, expected) in [
            (
                Some("session-model"),
                Some(Some(std::ffi::OsStr::new("session-model"))),
            ),
            (None, Some(None)),
        ] {
            let mut command = tokio::process::Command::new("ignored");
            command.env("GOOSE_MODEL", "stale-model");

            apply_model_environment(&mut command, model_name);

            assert_eq!(
                command
                    .as_std()
                    .get_envs()
                    .find_map(|(key, value)| (key == "GOOSE_MODEL").then_some(value)),
                expected
            );
        }
    }

    #[test]
    fn session_environment_is_set_or_removed() {
        for (session_id, expected) in [
            (
                Some("session-123"),
                Some(Some(std::ffi::OsStr::new("session-123"))),
            ),
            (None, Some(None)),
        ] {
            let mut command = tokio::process::Command::new("ignored");
            command.env("AGENT_SESSION_ID", "stale-session");

            apply_session_environment(&mut command, session_id);

            assert_eq!(
                command
                    .as_std()
                    .get_envs()
                    .find_map(|(key, value)| (key == "AGENT_SESSION_ID").then_some(value)),
                expected
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn flatpak_environment_is_set_or_unset() {
        for (session_id, model_name, expected) in [
            (
                Some("session-123"),
                Some("session-model"),
                vec![
                    "--env=AGENT_SESSION_ID=session-123",
                    "--env=GOOSE_MODEL=session-model",
                ],
            ),
            (
                None,
                None,
                vec!["--unset-env=AGENT_SESSION_ID", "--unset-env=GOOSE_MODEL"],
            ),
        ] {
            let mut command = tokio::process::Command::new("flatpak-spawn");

            apply_flatpak_session_environment(&mut command, session_id);
            apply_flatpak_model_environment(&mut command, model_name);

            assert_eq!(
                command
                    .as_std()
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_kills_child_on_cancellation() {
        let tool = ShellTool::new_for_test().unwrap();
        let token = CancellationToken::new();
        let token_clone = token.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            token_clone.cancel();
        });

        let start = std::time::Instant::now();
        let result = tool
            .shell_with_cwd(
                ShellParams {
                    command: "sleep 30".to_string(),
                    timeout_secs: None,
                },
                None,
                None,
                token,
            )
            .await;

        assert!(
            start.elapsed().as_secs() < 5,
            "shell should return quickly after cancellation, not wait for the command"
        );
        let shell_output = extract_shell_output(&result);
        assert!(
            shell_output.exit_code.is_none(),
            "cancelled process should have no exit code"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shell_flavor_detects_nushell_names() {
        assert_eq!(unix_shell_flavor("nu"), UnixShellFlavor::Nushell);
        assert_eq!(unix_shell_flavor("nushell"), UnixShellFlavor::Nushell);
        assert_eq!(
            unix_shell_flavor("/etc/profiles/per-user/can/bin/nu"),
            UnixShellFlavor::Nushell
        );
        assert_eq!(unix_shell_flavor("/bin/bash"), UnixShellFlavor::Posix);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_login_shell_command_args_use_nushell_probe() {
        assert_eq!(
            unix_login_shell_command_args("nu"),
            ["-l", "-i", "-c", "print ($env.PATH | str join (char esep))"]
        );
        assert_eq!(
            unix_login_shell_command_args("/bin/bash"),
            ["-l", "-i", "-c", "echo $PATH"]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shell_command_args_wrap_commands_for_execution() {
        assert_eq!(unix_shell_command_args("ls -la"), ["-c", "ls -la"]);
    }

    #[test]
    fn render_output_returns_full_output_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        let input = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let result = render_output(&input, "test", dir.path()).unwrap();
        assert_eq!(result.text, input);
        assert!(result.truncation.is_none());
    }

    #[test]
    fn render_output_shows_empty_message() {
        let dir = tempfile::tempdir().unwrap();
        let result = render_output("", "test", dir.path()).unwrap();
        assert_eq!(result.text, "(no output)");
        assert!(result.truncation.is_none());
    }

    #[test]
    fn render_output_truncates_when_lines_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let input = (0..2500)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let result = render_output(&input, "test_lines", dir.path()).unwrap();
        let preview = &result.text;

        assert_eq!(preview.lines().count(), OUTPUT_PREVIEW_LINES);
        assert!(preview.starts_with("line 2450"));
        assert!(preview.contains("line 2499"));

        let info = result
            .truncation
            .as_ref()
            .expect("expected truncation info");
        assert!(info.reason.contains("2000 line limit"));
        assert!(info.reason.contains("2500 lines total"));

        let notice = truncation_notice(info);
        assert!(notice.contains("Full output saved to"));
    }

    #[test]
    fn render_output_truncates_when_bytes_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let long_line = "x".repeat(1000);
        let input = (0..100)
            .map(|_| long_line.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(input.len() > OUTPUT_LIMIT_BYTES);
        assert!(input.lines().count() <= OUTPUT_LIMIT_LINES);

        let result = render_output(&input, "test_bytes", dir.path()).unwrap();
        let info = result
            .truncation
            .as_ref()
            .expect("expected truncation info");
        assert!(info.reason.contains("byte limit"));
        assert!(info.reason.contains("bytes total"));

        let notice = truncation_notice(info);
        assert!(notice.contains("Full output saved to"));
    }

    #[test]
    fn render_output_preview_is_byte_bounded_for_giant_lines() {
        let dir = tempfile::tempdir().unwrap();
        let input = "x".repeat(200_000);

        let result = render_output(&input, "test_giant_line", dir.path()).unwrap();

        assert!(result.text.len() <= OUTPUT_PREVIEW_BYTES);
        let info = result
            .truncation
            .as_ref()
            .expect("expected truncation info");
        assert!(info.reason.contains("byte limit"));
        assert_eq!(std::fs::read_to_string(&info.path).unwrap().len(), 200_000);
    }

    #[test]
    fn truncate_preview_bytes_respects_char_boundaries() {
        let preview = "é".repeat(OUTPUT_PREVIEW_BYTES);
        let truncated = truncate_preview_bytes(preview);
        assert!(truncated.len() <= OUTPUT_PREVIEW_BYTES);
        assert!(truncated.chars().all(|c| c == 'é'));
    }

    #[test]
    fn save_full_output_reuses_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path1 = save_full_output("first", "test_reuse", dir.path()).unwrap();
        let path2 = save_full_output("second", "test_reuse", dir.path()).unwrap();
        assert_eq!(path1, path2);
        // Note: we intentionally don't assert file content here because
        // parallel tests (render_output_truncates_*) share the same static
        // temp file and can overwrite the content between our write and read.
    }

    #[test]
    fn save_full_output_uses_separate_files_per_label() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = save_full_output("aaa", "label_a", dir.path()).unwrap();
        let path_b = save_full_output("bbb", "label_b", dir.path()).unwrap();
        assert_ne!(path_a, path_b);
        assert_eq!(std::fs::read_to_string(&path_a).unwrap(), "aaa");
        assert_eq!(std::fs::read_to_string(&path_b).unwrap(), "bbb");
    }

    #[test]
    fn call_index_cycles_through_slots() {
        let tool = ShellTool::new_for_test().unwrap();
        for _cycle in 0..3 {
            for expected in 0..OUTPUT_SLOTS {
                let slot = tool.call_index.fetch_add(1, Ordering::Relaxed) % OUTPUT_SLOTS;
                assert_eq!(slot, expected);
            }
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_rejects_newline_in_command() {
        for command in ["echo a\necho b", "echo a\r\necho b", "echo a\recho b"] {
            let tool = ShellTool::new_for_test().unwrap();
            let result = tool
                .shell(ShellParams {
                    command: command.to_string(),
                    timeout_secs: None,
                })
                .await;
            assert_eq!(
                result.is_error,
                Some(true),
                "expected error for {command:?}"
            );
            assert!(
                extract_text(&result).contains("cmd.exe"),
                "error should mention cmd.exe for {command:?}"
            );
        }
    }

    #[test]
    fn concurrent_calls_get_distinct_slots() {
        let tool = ShellTool::new_for_test().unwrap();
        let mut slots: Vec<usize> = (0..OUTPUT_SLOTS)
            .map(|_| tool.call_index.fetch_add(1, Ordering::Relaxed) % OUTPUT_SLOTS)
            .collect();
        slots.sort();
        let expected: Vec<usize> = (0..OUTPUT_SLOTS).collect();
        assert_eq!(slots, expected);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_does_not_hang_on_backgrounded_process() {
        struct KillOnDrop(String);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &self.0])
                    .status();
            }
        }

        let tool = ShellTool::new_for_test().unwrap();
        let start = std::time::Instant::now();
        let result = tool
            .shell(ShellParams {
                command: "echo before && sleep 300 & echo bgpid:$! && echo after".to_string(),
                timeout_secs: None,
            })
            .await;

        assert!(
            start.elapsed().as_secs() < 10,
            "shell tool should return quickly, not wait for backgrounded sleep"
        );
        assert_eq!(result.is_error, Some(false));
        let text = extract_text(&result);
        let shell_output = extract_shell_output(&result);
        let background_pid = text
            .lines()
            .find_map(|line| line.strip_prefix("bgpid:"))
            .map(str::trim)
            .expect("expected bgpid in output");
        let _cleanup = KillOnDrop(background_pid.to_string());
        assert!(
            shell_output.output_truncated,
            "backgrounded process should set output_truncated"
        );
        assert!(
            shell_output.output_collection_error.is_none(),
            "timeout-based truncation should not set output collection error"
        );
        assert!(
            text.contains("before"),
            "should capture output before background cmd"
        );
        assert!(
            text.contains("after"),
            "should capture output after background cmd"
        );
    }

    #[test]
    fn resolve_shell_timeout_prefers_explicit_value() {
        assert_eq!(resolve_shell_timeout(Some(42)), 42);
    }

    #[test]
    fn resolve_shell_timeout_falls_back_to_a_bound_when_absent() {
        // The key behavioral guarantee: an omitted timeout no longer means
        // "run forever" — it resolves to the default extension timeout.
        assert!(resolve_shell_timeout(None) > 0);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn shell_kills_hanging_command_after_explicit_timeout() {
        let tool = ShellTool::new_for_test().unwrap();
        let start = std::time::Instant::now();
        let result = tool
            .shell(ShellParams {
                command: "sleep 30".to_string(),
                timeout_secs: Some(1),
            })
            .await;

        assert!(
            start.elapsed().as_secs() < 10,
            "shell should return shortly after the timeout, not wait for the command"
        );
        assert_eq!(result.is_error, Some(true));
        let shell_output = extract_shell_output(&result);
        assert!(shell_output.timed_out, "command should be marked timed_out");
        assert!(
            shell_output.exit_code.is_none(),
            "killed process should have no exit code"
        );
        assert!(extract_text(&result).contains("Command timed out after 1 seconds"));
    }
}
