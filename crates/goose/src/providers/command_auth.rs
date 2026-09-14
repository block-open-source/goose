use anyhow::Result;
use async_trait::async_trait;
use goose_providers::declarative::AuthConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::api_client::AuthProvider;
use crate::subprocess::configure_subprocess;

const DEFAULT_AUTH_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_STDERR_BYTES: usize = 500;

struct CachedCredential {
    token: String,
    fetched_at: Instant,
}

/// Fetches a credential by running a user-configured command and caches the
/// result for `refresh_interval` before re-running it. Runtime counterpart of
/// `DeclarativeProviderConfig::auth`, for custom providers whose credentials
/// are short-lived and issued by an external script rather than a static key.
pub struct CommandAuthProvider {
    command: String,
    args: Vec<String>,
    refresh_interval: Duration,
    timeout: Duration,
    /// Working directory for the command, and the base a relative `command`
    /// resolves against. Captured once at construction (defaults to goose's
    /// current directory at that point), not re-read per invocation.
    cwd: PathBuf,
    header_name: String,
    header_value_prefix: String,
    cached: Arc<RwLock<Option<CachedCredential>>>,
}

impl CommandAuthProvider {
    pub fn new(
        auth_config: &AuthConfig,
        header_name: impl Into<String>,
        header_value_prefix: impl Into<String>,
    ) -> Self {
        // Made absolute up front: `cwd` doubles as both `current_dir` for the
        // spawned process and the join base in `resolve_program`, and a
        // relative value would otherwise be applied twice (once by us, once
        // by the OS resolving a relative program path against the process's
        // now-`current_dir`-ed working directory).
        let cwd = auth_config
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir()
            .map(|base| {
                if cwd.is_absolute() {
                    cwd.clone()
                } else {
                    base.join(&cwd)
                }
            })
            .unwrap_or(cwd);
        Self {
            command: auth_config.command.clone(),
            args: auth_config.args.clone(),
            refresh_interval: Duration::from_secs(auth_config.refresh_interval),
            timeout: auth_config
                .timeout_seconds
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_AUTH_COMMAND_TIMEOUT),
            cwd,
            header_name: header_name.into(),
            header_value_prefix: header_value_prefix.into(),
            cached: Arc::new(RwLock::new(None)),
        }
    }

    async fn fetch_token(&self) -> Result<String> {
        // Spawned directly, never through a shell, so `args` is never
        // shell-interpolated. Inherits goose's full environment, since the
        // same user configures goose and writes the script.
        let program = resolve_program(&self.command, &self.cwd);
        let mut command = tokio::process::Command::new(&program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            // No stdin, so a script that unexpectedly tries to prompt fails
            // fast instead of hanging on the parent's stdin. `kill_on_drop`
            // is a fallback for exit paths other than the timeout below,
            // which kills the whole process group explicitly — the direct
            // child alone wouldn't take a shell pipeline's descendants with
            // it, since `configure_subprocess` puts the child in its own
            // new process group.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        configure_subprocess(&mut command);

        let child = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to run auth command '{}': {}", self.command, e))?;
        #[cfg(unix)]
        let pid = child.id();

        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| {
                anyhow::anyhow!("failed to run auth command '{}': {}", self.command, e)
            })?,
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = pid {
                    // SAFETY: signals only the process group `configure_subprocess`
                    // put this child in (negative pid), never an unrelated group.
                    unsafe {
                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    }
                }
                anyhow::bail!(
                    "auth command '{}' timed out after {:?}",
                    self.command,
                    self.timeout
                );
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "auth command '{}' exited with {}: {}",
                self.command,
                output.status,
                truncate(&stderr, MAX_STDERR_BYTES)
            );
        }

        let token = String::from_utf8(output.stdout)
            .map_err(|_| {
                anyhow::anyhow!(
                    "auth command '{}' wrote non-UTF-8 data to stdout",
                    self.command
                )
            })?
            .trim()
            .to_string();
        if token.is_empty() {
            anyhow::bail!(
                "auth command '{}' produced an empty credential",
                self.command
            );
        }

        Ok(token)
    }

    /// `refresh_interval: 0` means "never expire proactively" — matching
    /// Codex's `refresh_interval_ms: 0` convention — so the credential is
    /// only refreshed reactively, via `refresh_credentials()` after an auth
    /// failure, not on a TTL.
    fn is_fresh(&self, cached: &CachedCredential) -> bool {
        self.refresh_interval.is_zero() || cached.fetched_at.elapsed() < self.refresh_interval
    }
}

/// A bare command name (no path separator) is left alone for `PATH` lookup;
/// an absolute path is used as-is; a relative path is resolved against `cwd`.
fn resolve_program(command: &str, cwd: &Path) -> PathBuf {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if path.components().count() > 1 {
        return cwd.join(path);
    }
    PathBuf::from(command)
}

fn truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or(s)
}

#[async_trait]
impl AuthProvider for CommandAuthProvider {
    async fn get_auth_header(&self) -> Result<(String, String)> {
        // Try read lock first for better concurrency
        if let Some(cached) = self.cached.read().await.as_ref() {
            if self.is_fresh(cached) {
                return Ok((
                    self.header_name.clone(),
                    format!("{}{}", self.header_value_prefix, cached.token),
                ));
            }
        }

        // Take write lock only if needed
        let mut guard = self.cached.write().await;

        // Double-check freshness after acquiring write lock
        if let Some(cached) = guard.as_ref() {
            if self.is_fresh(cached) {
                return Ok((
                    self.header_name.clone(),
                    format!("{}{}", self.header_value_prefix, cached.token),
                ));
            }
        }

        // Get a new token. No fallback to a stale cached token on failure:
        // that would surface as a confusing downstream 401 instead of a
        // clear "the refresh command failed" error.
        let token = self.fetch_token().await?;
        *guard = Some(CachedCredential {
            token: token.clone(),
            fetched_at: Instant::now(),
        });

        Ok((
            self.header_name.clone(),
            format!("{}{}", self.header_value_prefix, token),
        ))
    }

    async fn refresh_credentials(&self) -> Result<()> {
        *self.cached.write().await = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_config(
        command: impl Into<String>,
        args: Vec<impl Into<String>>,
        refresh_interval: u64,
    ) -> AuthConfig {
        AuthConfig {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            refresh_interval,
            timeout_seconds: None,
            cwd: None,
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

    #[cfg(unix)]
    fn sleep_seconds_command(seconds: u64) -> (&'static str, Vec<String>) {
        ("sleep", vec![seconds.to_string()])
    }
    #[cfg(windows)]
    fn sleep_seconds_command(seconds: u64) -> (&'static str, Vec<String>) {
        (
            "powershell",
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("Start-Sleep -Seconds {seconds}"),
            ],
        )
    }

    #[cfg(unix)]
    fn fail_command() -> (&'static str, Vec<&'static str>) {
        ("sh", vec!["-c", "exit 1"])
    }
    #[cfg(windows)]
    fn fail_command() -> (&'static str, Vec<&'static str>) {
        ("cmd", vec!["/C", "exit 1"])
    }

    #[cfg(unix)]
    fn fail_after_printing_command(text: &str) -> (&'static str, Vec<String>) {
        ("sh", vec!["-c".to_string(), format!("echo {text}; exit 1")])
    }
    #[cfg(windows)]
    fn fail_after_printing_command(text: &str) -> (&'static str, Vec<String>) {
        (
            "cmd",
            vec!["/C".to_string(), format!("echo {text} & exit 1")],
        )
    }

    #[tokio::test]
    async fn caches_token_within_refresh_interval() {
        // A fresh value on every invocation, so two equal results back-to-back
        // prove the command was only run once.
        let (command, args) = distinct_value_command();
        let provider = CommandAuthProvider::new(
            &auth_config(command, args, 3600),
            "Authorization",
            "Bearer ",
        );

        let (header, first) = provider.get_auth_header().await.unwrap();
        assert_eq!(header, "Authorization");
        let (_, second) = provider.get_auth_header().await.unwrap();

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn refetches_after_ttl_expires() {
        let (command, args) = distinct_value_command();
        let provider =
            CommandAuthProvider::new(&auth_config(command, args, 1), "Authorization", "Bearer ");

        let (_, first) = provider.get_auth_header().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let (_, second) = provider.get_auth_header().await.unwrap();

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn zero_refresh_interval_disables_proactive_refresh() {
        // Matches Codex's `refresh_interval_ms: 0` convention: the cache
        // never ages out on its own, only `refresh_credentials()` (the
        // reactive, on-401 path) invalidates it.
        let (command, args) = distinct_value_command();
        let provider =
            CommandAuthProvider::new(&auth_config(command, args, 0), "Authorization", "Bearer ");

        let (_, first) = provider.get_auth_header().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_, second) = provider.get_auth_header().await.unwrap();
        assert_eq!(first, second);

        provider.refresh_credentials().await.unwrap();
        let (_, third) = provider.get_auth_header().await.unwrap();
        assert_ne!(second, third);
    }

    #[tokio::test]
    async fn refresh_credentials_forces_refetch() {
        let (command, args) = distinct_value_command();
        let provider = CommandAuthProvider::new(
            &auth_config(command, args, 3600),
            "Authorization",
            "Bearer ",
        );

        let (_, first) = provider.get_auth_header().await.unwrap();
        provider.refresh_credentials().await.unwrap();
        let (_, second) = provider.get_auth_header().await.unwrap();

        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn enforces_timeout() {
        let (command, args) = sleep_seconds_command(5);
        let mut config = auth_config(command, args, 3600);
        config.timeout_seconds = Some(1);
        let provider = CommandAuthProvider::new(&config, "Authorization", "Bearer ");

        let start = Instant::now();
        let result = provider.get_auth_header().await;
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn errors_on_nonzero_exit_without_falling_back_to_stale_cache() {
        let (command, args) = fail_command();
        let provider = CommandAuthProvider::new(
            &auth_config(command, args, 3600),
            "Authorization",
            "Bearer ",
        );
        // Seed a valid cached credential, then invalidate it the same way a
        // 401 response does, so the next fetch attempt hits the failing
        // command instead of returning the still-fresh cached value.
        *provider.cached.write().await = Some(CachedCredential {
            token: "stale-token".to_string(),
            fetched_at: Instant::now(),
        });
        provider.refresh_credentials().await.unwrap();

        let result = provider.get_auth_header().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn error_message_does_not_leak_stdout() {
        let (command, args) = fail_after_printing_command("super-secret-token");
        let provider = CommandAuthProvider::new(
            &auth_config(command, args, 3600),
            "Authorization",
            "Bearer ",
        );

        let err = provider.get_auth_header().await.unwrap_err();
        assert!(!err.to_string().contains("super-secret-token"));
    }

    #[test]
    fn resolve_program_leaves_bare_names_for_path_lookup() {
        let resolved = resolve_program("aws", Path::new("/some/cwd"));
        assert_eq!(resolved, PathBuf::from("aws"));
    }

    #[test]
    fn resolve_program_leaves_absolute_paths_unchanged() {
        let resolved = resolve_program("/usr/local/bin/get-token.sh", Path::new("/some/cwd"));
        assert_eq!(resolved, PathBuf::from("/usr/local/bin/get-token.sh"));
    }

    #[test]
    fn resolve_program_joins_relative_paths_against_cwd() {
        let resolved = resolve_program("./scripts/get-token.sh", Path::new("/some/cwd"));
        assert_eq!(
            resolved,
            Path::new("/some/cwd").join("./scripts/get-token.sh")
        );
    }

    #[test]
    fn relative_cwd_is_resolved_to_absolute_before_use() {
        // A relative `cwd` must become absolute once, at construction, so it
        // isn't applied twice: once by `resolve_program`'s join, once by the
        // OS resolving a relative program path against the already
        // `current_dir`-ed child process.
        let mut config = auth_config("./get-token", Vec::<&str>::new(), 3600);
        config.cwd = Some("some/relative/dir".to_string());
        let provider = CommandAuthProvider::new(&config, "Authorization", "Bearer ");

        assert!(provider.cwd.is_absolute());
        assert_eq!(
            provider.cwd,
            std::env::current_dir().unwrap().join("some/relative/dir")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_runs_with_configured_cwd_and_resolves_relative_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("get-token.sh");
        std::fs::write(&script_path, "#!/bin/sh\npwd\n").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut config = auth_config("./get-token.sh", Vec::<&str>::new(), 3600);
        config.cwd = Some(dir.path().to_string_lossy().to_string());
        let provider = CommandAuthProvider::new(&config, "Authorization", "Bearer ");

        let (_, value) = provider.get_auth_header().await.unwrap();
        let token = value.strip_prefix("Bearer ").unwrap();
        assert_eq!(
            std::fs::canonicalize(token).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }
}
