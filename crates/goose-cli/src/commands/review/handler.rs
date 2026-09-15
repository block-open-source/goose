use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::session::{build_session, SessionBuilderConfig};

use goose::checks::{discover, DiscoveredReview};
use goose::subprocess::git_command;

use super::orchestrator::{
    emit_findings, run_checks_in_parallel, run_main_pass_in_parallel, Severity,
};
use super::prompt::{build_review_prompt, DEFAULT_REVIEW_PROMPT};

/// Options for `goose review`.
#[derive(Debug, Clone, Default)]
pub struct ReviewOptions {
    /// Diff range to review (e.g. `main...HEAD`). When `None`, falls back to
    /// the working tree vs. the inferred merge base / default branch.
    pub range: Option<String>,
    /// Path to a markdown file with a custom base review prompt. Overrides the
    /// embedded default prompt entirely.
    pub prompt_file: Option<PathBuf>,
    /// Default model used for the main review agent and for any check that
    /// does not declare its own `model:`.
    pub default_model: Option<String>,
    /// Provider for the main review agent.
    pub provider: Option<String>,
    /// Force every discovered check to run with this model, regardless of
    /// the check's own `model:` field.
    pub override_model: Option<String>,
    /// Default `turn-limit` for orchestrated main-pass subprocesses and for
    /// checks that do not declare their own. Does not cap the legacy
    /// `--no-orchestrate` in-process main agent.
    pub default_turn_limit: Option<usize>,
    /// Print the assembled prompt and discovered checks instead of dispatching
    /// the review.
    pub dry_run: bool,
    /// Suppress non-result output from the underlying agent.
    pub quiet: bool,
    /// Disable the Rust-driven parallel orchestrator and fall back to the
    /// single-prompt path that asks the main agent to delegate checks via
    /// `delegate(... async: true ...)`. Useful when comparing against the
    /// in-process behavior or running on a model that handles dispatch
    /// reliably on its own. Checks with an explicit tool allowlist require
    /// the default orchestrator and are rejected on this path.
    pub no_orchestrate: bool,
    /// Additional free-form instructions to prepend to the review (PR
    /// intent, commit-message context, etc.). Surfaced to both the main
    /// agent and every check subprocess.
    pub instructions: Option<String>,
    /// Restrict the review to a specific set of files (repo-relative).
    /// When non-empty, the diff sent to the agent is filtered to only
    /// include hunks for these paths.
    pub files: Vec<String>,
    /// Only run checks whose `name` is in this list. Empty means run all
    /// discovered checks (the default).
    pub check_filter: Vec<String>,
    /// Alternate directory to search for `.agents/checks/*.md` instead of
    /// the repo root.
    pub check_scope: Option<PathBuf>,
    /// Skip the main correctness pass and only run check subagents.
    pub checks_only: bool,
    /// Print only the diff summary; skip the full review.
    pub summary_only: bool,
    /// Minimum severity to display from check findings. Defaults to
    /// `medium`, matching Amp's CLI behavior of hiding `low` from
    /// the review output.
    pub severity: String,
}

/// Entry point for the `goose review` subcommand.
pub async fn handle_review(opts: ReviewOptions) -> Result<()> {
    let repo_root = find_repo_root().context("not inside a git repository")?;
    let untracked_root = opts
        .range
        .is_none()
        .then(|| open_untracked_root(&repo_root))
        .transpose()?;

    // Validate `--severity` once, up front, so a bogus value fails fast
    // regardless of which orchestration path we end up taking.
    let sev_str = if opts.severity.is_empty() {
        "medium"
    } else {
        opts.severity.as_str()
    };
    let min_sev: Severity = sev_str
        .parse()
        .map_err(|e: String| anyhow!("--severity: {e}"))?;

    let mut touched = touched_files(&repo_root, opts.range.as_deref(), &opts.files)?;
    let mut diff = collect_diff(&repo_root, opts.range.as_deref(), &opts.files)?;

    // Without an explicit `--range`, `git diff HEAD` excludes untracked
    // files entirely — brand-new files would silently miss the review.
    // Synthesize a `new file` diff for each so the main pass and the
    // checks see them.
    if let Some(untracked_root) = untracked_root.as_ref() {
        let untracked = untracked_files(untracked_root, &opts.files)?;
        if !untracked.is_empty() {
            let untracked_diff = synthesize_untracked_diff(untracked_root, &untracked)?;
            diff.push_str(&untracked_diff);
            for u in untracked {
                if !touched.contains(&u) {
                    touched.push(u);
                }
            }
        }
    }
    drop(untracked_root);

    if diff.trim().is_empty() {
        eprintln!("goose review: no changes to review");
        return Ok(());
    }

    // `--summary-only` short-circuits everything else: print `git
    // diff --stat` and return without calling the agent. Mirrors
    // `amp review --summary-only`.
    if opts.summary_only {
        let summary = collect_diff_stat(&repo_root, opts.range.as_deref(), &opts.files)?;
        print!("{}", summary);
        return Ok(());
    }

    // `--check-scope` overrides where we look for `.agents/checks/*.md`,
    // otherwise discovery walks from the repo root + every directory on
    // the path of a touched file.
    let discovery_root = opts.check_scope.as_deref().unwrap_or(&repo_root);
    // `touched` is repo-relative; rebase to discovery_root so candidate
    // scope walking doesn't double-prefix `<scope>/api/...` for files
    // already living under the scope.
    let discovery_touched = rebase_touched_to_scope(&repo_root, discovery_root, &touched);
    let discovered = discover(discovery_root, &discovery_touched)?;
    let discovered = filter_checks(discovered, &opts.check_filter);
    if !opts.quiet {
        print_discovered_summary(&discovered);
    }

    let base_prompt = match &opts.prompt_file {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("read --prompt file {}", path.display()))?,
        None => DEFAULT_REVIEW_PROMPT.to_string(),
    };

    let use_orchestrator = !opts.no_orchestrate;

    // Reviewer instructions are also injected into every per-file
    // main-pass subprocess and every per-check subprocess. To avoid
    // duplicating them, only prepend to the base prompt for the legacy
    // single-prompt (`--no-orchestrate`) path.
    let base_prompt = if use_orchestrator {
        base_prompt
    } else {
        prepend_instructions(&base_prompt, opts.instructions.as_deref())
    };

    // In orchestrator mode, the main pass runs as N parallel subprocesses
    // (one per touched file) — checks run as parallel subprocesses too —
    // so the assembled prompt only matters for the legacy in-process path.
    let main_prompt_discovered = if use_orchestrator {
        DiscoveredReview::default()
    } else {
        discovered.clone()
    };
    let prompt = build_review_prompt(
        &base_prompt,
        &main_prompt_discovered,
        &diff,
        opts.default_model.as_deref(),
        opts.override_model.as_deref(),
        opts.default_turn_limit,
    );

    if opts.dry_run {
        println!("{}", prompt);
        if use_orchestrator {
            println!(
                "\n# orchestrator: {} check(s) would run as parallel subprocesses",
                discovered.checks.len()
            );
            println!("# orchestrator: main pass would fan out one subprocess per touched file");
        }
        return Ok(());
    }

    if !use_orchestrator {
        // Legacy in-process path (--no-orchestrate). Useful for comparing
        // against orchestrated wall clock and for models that handle
        // delegation reliably on their own.
        if opts.checks_only {
            // The legacy path runs everything as a single agent prompt,
            // so it has no way to "skip the main pass". Fall back to the
            // orchestrator's check-runner (which IS able to run checks
            // in isolation) instead of silently no-op'ing.
            let check_results = run_checks_in_parallel(&discovered.checks, &diff, &opts).await;
            let mut total_emitted = 0usize;
            let mut total_seen = 0usize;
            for findings in &check_results {
                total_seen += findings.len();
                total_emitted += emit_findings(findings, min_sev);
            }
            if !opts.quiet {
                let suppressed = total_seen.saturating_sub(total_emitted);
                eprintln!(
                    "goose review: emitted {total_emitted} finding(s) from {} check(s) ({suppressed} hidden below severity={:?})",
                    discovered.checks.len(),
                    min_sev
                );
            }
            return Ok(());
        }
        ensure_legacy_check_tools_are_unrestricted(&discovered)?;
        let mut session = build_session(SessionBuilderConfig {
            session_id: None,
            no_session: true,
            no_profile: true,
            builtins: vec!["developer".to_string(), "summon".to_string()],
            provider: opts.provider.clone(),
            model: opts.default_model.clone(),
            quiet: opts.quiet,
            output_format: "text".to_string(),
            ..SessionBuilderConfig::default()
        })
        .await;
        return session.headless(prompt).await;
    }

    // Orchestrated mode: run the main correctness pass (per-file
    // parallel subprocesses) and the discovered checks (one subprocess
    // each, capped at MAX_WORKERS) concurrently. Wall clock is bounded
    // by `max(slowest_main_file, slowest_check)` instead of scaling
    // with diff size or check count.
    let main_findings_fut = async {
        if opts.checks_only {
            Vec::new()
        } else {
            run_main_pass_in_parallel(&diff, &base_prompt, &opts).await
        }
    };
    let checks_fut = run_checks_in_parallel(&discovered.checks, &diff, &opts);
    let (main_findings, check_results) = tokio::join!(main_findings_fut, checks_fut);

    let mut total_emitted = 0usize;
    let mut total_seen = main_findings.len();
    total_emitted += emit_findings(&main_findings, min_sev);
    for findings in &check_results {
        total_seen += findings.len();
        total_emitted += emit_findings(findings, min_sev);
    }
    if !opts.quiet {
        let suppressed = total_seen.saturating_sub(total_emitted);
        let main_pass_label = if opts.checks_only { "skipped" } else { "ran" };
        if suppressed == 0 {
            eprintln!(
                "goose review: orchestrator emitted {total_emitted} finding(s) from {} check(s) (main: {main_pass_label}, {} finding(s))",
                discovered.checks.len(),
                main_findings.len()
            );
        } else {
            eprintln!(
                "goose review: orchestrator emitted {total_emitted} finding(s) from {} check(s) (main: {main_pass_label}, {} finding(s); {suppressed} hidden below severity={:?})",
                discovered.checks.len(),
                main_findings.len(),
                min_sev
            );
        }
    }

    Ok(())
}

/// Restrict a discovered review to the named checks (no-op when the
/// filter is empty). Mirrors `amp review --check-filter`.
fn filter_checks(discovered: DiscoveredReview, names: &[String]) -> DiscoveredReview {
    if names.is_empty() {
        return discovered;
    }
    let allow: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    DiscoveredReview {
        checks: discovered
            .checks
            .into_iter()
            .filter(|c| allow.contains(c.name.as_str()))
            .collect(),
    }
}

fn ensure_legacy_check_tools_are_unrestricted(discovered: &DiscoveredReview) -> Result<()> {
    let restricted: Vec<&str> = discovered
        .checks
        .iter()
        .filter(|check| check.tools.is_some())
        .map(|check| check.name.as_str())
        .collect();
    if restricted.is_empty() {
        return Ok(());
    }

    bail!(
        "--no-orchestrate cannot enforce per-check tool allowlists for: {}; rerun without --no-orchestrate",
        restricted.join(", ")
    )
}

/// Prepend a free-form `--instructions <text>` block to the base prompt
/// so it is visible to both the main agent and (via the orchestrator)
/// every per-check subprocess.
fn prepend_instructions(base_prompt: &str, instructions: Option<&str>) -> String {
    match instructions {
        Some(text) if !text.trim().is_empty() => {
            format!(
                "## Reviewer instructions\n\n{}\n\n{}",
                text.trim(),
                base_prompt
            )
        }
        _ => base_prompt.to_string(),
    }
}

fn print_discovered_summary(d: &DiscoveredReview) {
    if d.checks.is_empty() {
        eprintln!("goose review: no checks or REVIEW.md rules discovered");
        return;
    }
    eprintln!("goose review: discovered {} check(s):", d.checks.len());
    for c in &d.checks {
        let scope = if c.scope_dir.is_empty() {
            "<root>"
        } else {
            &c.scope_dir
        };
        eprintln!("  - {} (scope: {})", c.name, scope);
    }
}

fn find_repo_root() -> Result<PathBuf> {
    let out = git_command()
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to invoke git")?;
    if !out.status.success() {
        bail!(
            "git rev-parse --show-toplevel failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let path = String::from_utf8(out.stdout)?.trim().to_string();
    Ok(PathBuf::from(path))
}

/// Configure a `git` Command to disable quoting of non-ASCII paths.
/// Without this, paths containing non-ASCII bytes come back as quoted
/// C-style escapes (`"dir/\303\251.txt"`), which downstream parsers
/// would have to round-trip-decode just to spell the filename. We turn
/// it off everywhere so callers always get clean UTF-8 paths.
fn review_git_command(repo_root: &Path) -> Command {
    let mut cmd = git_command();
    cmd.current_dir(repo_root)
        .args(["-c", "core.quotePath=off"]);
    cmd
}

fn review_diff_command(repo_root: &Path) -> Command {
    let mut cmd = review_git_command(repo_root);
    cmd.args(["diff", "--no-ext-diff", "--no-textconv"]);
    cmd
}

fn touched_files(repo_root: &Path, range: Option<&str>, files: &[String]) -> Result<Vec<String>> {
    let mut cmd = review_diff_command(repo_root);
    cmd.arg("--name-only");
    match range {
        Some(r) => {
            cmd.arg(r);
        }
        None => {
            cmd.arg("HEAD");
        }
    }
    if !files.is_empty() {
        cmd.arg("--");
        for f in files {
            cmd.arg(f);
        }
    }
    let out = cmd.output().context("git diff --name-only failed")?;
    if !out.status.success() {
        bail!(
            "git diff --name-only failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect())
}

fn collect_diff(repo_root: &Path, range: Option<&str>, files: &[String]) -> Result<String> {
    let mut cmd = review_diff_command(repo_root);
    match range {
        Some(r) => {
            cmd.arg(r);
        }
        None => {
            cmd.arg("HEAD");
        }
    }
    if !files.is_empty() {
        cmd.arg("--");
        for f in files {
            cmd.arg(f);
        }
    }
    let out = cmd.output().context("git diff failed")?;
    if !out.status.success() {
        bail!("git diff failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    String::from_utf8(out.stdout).map_err(|e| anyhow!("git diff returned non-UTF8 output: {e}"))
}

fn collect_diff_stat(repo_root: &Path, range: Option<&str>, files: &[String]) -> Result<String> {
    let mut cmd = review_diff_command(repo_root);
    cmd.arg("--stat");
    match range {
        Some(r) => {
            cmd.arg(r);
        }
        None => {
            cmd.arg("HEAD");
        }
    }
    if !files.is_empty() {
        cmd.arg("--");
        for f in files {
            cmd.arg(f);
        }
    }
    let out = cmd.output().context("git diff --stat failed")?;
    if !out.status.success() {
        bail!(
            "git diff --stat failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout)
        .map_err(|e| anyhow!("git diff --stat returned non-UTF8 output: {e}"))
}

/// List untracked-but-not-ignored files in `repo_root`. Used to expose
/// brand-new files to the review when no `--range` is given (default
/// `git diff HEAD` would silently drop them).
fn untracked_files(repo_root: &UntrackedRoot, files: &[String]) -> Result<Vec<String>> {
    let mut cmd = untracked_git_command(repo_root)?;
    cmd.args(["ls-files", "--others", "--exclude-standard"]);
    if !files.is_empty() {
        cmd.arg("--");
        for f in files {
            cmd.arg(f);
        }
    }
    let out = cmd.output().context("git ls-files failed")?;
    if !out.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect())
}

fn validated_relative_components(path: &Path) -> std::io::Result<Vec<&std::ffi::OsStr>> {
    use std::io::{Error, ErrorKind};
    use std::path::Component;

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => components.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "untracked path must be relative to the repository",
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "untracked path must name a file",
        ));
    }

    Ok(components)
}

#[cfg(unix)]
struct UntrackedRoot(fs::File);

#[cfg(windows)]
struct UntrackedRoot {
    directory: fs::File,
    path: PathBuf,
    _anchors: Vec<fs::File>,
}

#[cfg(not(any(unix, windows)))]
struct UntrackedRoot(PathBuf);

#[cfg(unix)]
fn untracked_git_command(repo_root: &UntrackedRoot) -> Result<Command> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let directory = repo_root.0.try_clone()?;
    let mut command = git_command();
    command.args(["-c", "core.quotePath=off"]);
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(directory.as_raw_fd()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

#[cfg(windows)]
fn untracked_git_command(repo_root: &UntrackedRoot) -> Result<Command> {
    Ok(review_git_command(&repo_root.path))
}

#[cfg(not(any(unix, windows)))]
fn untracked_git_command(repo_root: &UntrackedRoot) -> Result<Command> {
    Ok(review_git_command(&repo_root.0))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(all(unix, target_vendor = "apple"))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_SEARCH | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android")),
    not(target_vendor = "apple")
))]
fn directory_traversal_flags() -> libc::c_int {
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

#[cfg(unix)]
fn open_untracked_root(repo_root: &Path) -> std::io::Result<UntrackedRoot> {
    open_untracked_root_with_hook(repo_root, |_| {})
}

#[cfg(unix)]
fn open_untracked_root_with_hook(
    repo_root: &Path,
    mut after_opened_component: impl FnMut(&Path),
) -> std::io::Result<UntrackedRoot> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Component;

    let mut options = fs::OpenOptions::new();
    options.read(true).custom_flags(directory_traversal_flags());
    let mut directory = options.open(Path::new("/"))?;
    let mut opened_path = PathBuf::from("/");
    let mut saw_root = false;
    for component in repo_root.components() {
        match component {
            Component::RootDir if !saw_root => saw_root = true,
            Component::Normal(component) if saw_root => {
                directory = open_at(&directory, component, directory_traversal_flags())?;
                opened_path.push(component);
                after_opened_component(&opened_path);
            }
            Component::CurDir if saw_root => {}
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "repository root must be an absolute normalized path",
                ));
            }
        }
    }
    if !saw_root || opened_path != repo_root {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "repository root must be an absolute normalized path",
        ));
    }
    if !directory.metadata()?.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "repository root is not a directory",
        ));
    }
    Ok(UntrackedRoot(directory))
}

#[cfg(windows)]
fn open_untracked_root(repo_root: &Path) -> std::io::Result<UntrackedRoot> {
    open_untracked_root_with_hook(repo_root, |_| {})
}

#[cfg(windows)]
fn open_untracked_root_with_hook(
    repo_root: &Path,
    mut after_opened_component: impl FnMut(&Path),
) -> std::io::Result<UntrackedRoot> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::OpenOptionsExt;
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use winapi::um::winnt::{
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
    };

    let root_anchor = repo_root
        .ancestors()
        .last()
        .filter(|path| path.has_root())
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "repository root must be an absolute normalized path",
            )
        })?;
    let relative = repo_root.strip_prefix(root_anchor).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "repository root must be an absolute normalized path",
        )
    })?;
    let components = if relative.as_os_str().is_empty() {
        Vec::new()
    } else {
        validated_relative_components(relative)?
    };

    let mut options = fs::OpenOptions::new();
    options
        .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let mut directory = options.open(root_anchor)?;
    let root_metadata = directory.metadata()?;
    if windows_metadata_is_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "repository root is not a regular directory",
        ));
    }
    let mut opened_path = root_anchor.to_path_buf();
    let mut anchors = Vec::new();
    for component in components {
        let next = windows_open_at(&directory, component, true, false)?;
        anchors.push(directory);
        directory = next;
        let metadata = directory.metadata()?;
        if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "repository root ancestor is not a regular directory",
            ));
        }
        opened_path.push(component);
        after_opened_component(&opened_path);
    }
    if opened_path != repo_root {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "repository root must be an absolute normalized path",
        ));
    }
    Ok(UntrackedRoot {
        directory,
        path: repo_root.to_path_buf(),
        _anchors: anchors,
    })
}

#[cfg(not(any(unix, windows)))]
fn open_untracked_root(_repo_root: &Path) -> std::io::Result<UntrackedRoot> {
    Ok(UntrackedRoot(_repo_root.to_path_buf()))
}

#[cfg(unix)]
fn read_untracked_content(
    repo_root: &UntrackedRoot,
    path: &Path,
) -> std::io::Result<Option<(&'static str, String)>> {
    read_untracked_content_with_hook(repo_root, path, |_| {})
}

#[cfg(unix)]
fn read_untracked_content_with_hook(
    repo_root: &UntrackedRoot,
    path: &Path,
    mut after_opened_ancestor: impl FnMut(&Path),
) -> std::io::Result<Option<(&'static str, String)>> {
    use std::io::{Error, ErrorKind, Read};
    use std::os::unix::ffi::OsStringExt;

    let components = validated_relative_components(path)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = repo_root.0.try_clone()?;
    let mut opened_path = PathBuf::new();

    for ancestor in ancestors {
        directory = open_at(&directory, ancestor, directory_traversal_flags())?;
        opened_path.push(ancestor);
        after_opened_ancestor(&opened_path);
    }

    match open_at(
        &directory,
        file_name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    ) {
        Ok(mut file) => {
            if !file.metadata()?.is_file() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "untracked path is not a regular file",
                ));
            }
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            Ok(Some(("100644", content)))
        }
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            let target = read_link_at(&directory, file_name)?;
            let target = std::ffi::OsString::from_vec(target);
            let Some(target) = target.to_str() else {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "untracked symlink target is not UTF-8",
                ));
            };
            Ok(Some(("120000", target.to_string())))
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn open_at(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    flags: libc::c_int,
) -> std::io::Result<fs::File> {
    use std::ffi::CString;
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "untracked path contains a NUL byte",
        )
    })?;
    // SAFETY: openat does not retain the name pointer, and no creation flag requiring a mode is set.
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor on success.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn read_link_at(directory: &fs::File, name: &std::ffi::OsStr) -> std::io::Result<Vec<u8>> {
    use std::ffi::CString;
    use std::io::{Error, ErrorKind};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "untracked path contains a NUL byte",
        )
    })?;
    let mut target = vec![0; 256];
    loop {
        // SAFETY: readlinkat does not retain either pointer and writes at most target.len() bytes.
        let length = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                name.as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if length < 0 {
            return Err(Error::last_os_error());
        }
        let length = length as usize;
        if length < target.len() {
            target.truncate(length);
            return Ok(target);
        }
        target.resize(target.len() * 2, 0);
    }
}

#[cfg(windows)]
fn read_untracked_content(
    repo_root: &UntrackedRoot,
    path: &Path,
) -> std::io::Result<Option<(&'static str, String)>> {
    read_untracked_content_with_hook(repo_root, path, |_| {})
}

#[cfg(windows)]
fn read_untracked_content_with_hook(
    repo_root: &UntrackedRoot,
    path: &Path,
    mut after_opened_ancestor: impl FnMut(&Path),
) -> std::io::Result<Option<(&'static str, String)>> {
    use std::io::{Error, ErrorKind, Read};

    let components = validated_relative_components(path)?;
    let (file_name, ancestors) = components.split_last().unwrap();
    let mut directory = repo_root.directory.try_clone()?;
    let mut opened_path = PathBuf::new();

    for ancestor in ancestors {
        directory = windows_open_at(&directory, ancestor, true, true)?;
        let metadata = directory.metadata()?;
        if windows_metadata_is_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "untracked path ancestor is not a regular directory",
            ));
        }
        opened_path.push(ancestor);
        after_opened_ancestor(&opened_path);
    }

    let mut file = windows_open_at(&directory, file_name, false, true)?;
    let metadata = file.metadata()?;
    if windows_metadata_is_reparse_point(&metadata) {
        return Ok(windows_read_symlink_target(&file)?.map(|target| ("120000", target)));
    }
    if !metadata.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "untracked path is not a regular file",
        ));
    }

    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(Some(("100644", content)))
}

#[cfg(windows)]
fn windows_open_at(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    directory_only: bool,
    allow_delete: bool,
) -> std::io::Result<fs::File> {
    use ntapi::ntioapi::{
        NtCreateFile, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT, IO_STATUS_BLOCK,
    };
    use std::io::{Error, ErrorKind};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use winapi::shared::ntdef::{
        HANDLE, NT_SUCCESS, OBJECT_ATTRIBUTES, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
    };
    use winapi::um::winnt::{
        FILE_GENERIC_READ, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
    };

    let mut name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "untracked path component is too long",
            )
        })?;
    let mut unicode_name = UNICODE_STRING {
        Length: name_bytes,
        MaximumLength: name_bytes,
        Buffer: name.as_mut_ptr(),
    };
    let mut attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle() as HANDLE,
        ObjectName: &mut unicode_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null_mut(),
        SecurityQualityOfService: std::ptr::null_mut(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: IO_STATUS_BLOCK is a plain C data structure initialized before the synchronous call.
    let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut create_options = FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
    if directory_only {
        create_options |= FILE_DIRECTORY_FILE;
    }
    let mut share_access = FILE_SHARE_READ | FILE_SHARE_WRITE;
    if allow_delete {
        share_access |= FILE_SHARE_DELETE;
    }
    let desired_access = if directory_only {
        FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE
    } else {
        FILE_GENERIC_READ
    };
    // SAFETY: all pointers reference initialized values for the duration of the synchronous call.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &mut attributes,
            &mut io_status,
            std::ptr::null_mut(),
            0,
            share_access,
            FILE_OPEN,
            create_options,
            std::ptr::null_mut(),
            0,
        )
    };
    if !NT_SUCCESS(status) {
        return Err(windows_nt_status_error(status));
    }
    // SAFETY: NtCreateFile returned a new owned handle on success.
    Ok(unsafe { fs::File::from_raw_handle(handle.cast()) })
}

#[cfg(windows)]
fn windows_metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use winapi::um::winnt::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn windows_read_symlink_target(file: &fs::File) -> std::io::Result<Option<String>> {
    use ntapi::ntioapi::{NtFsControlFile, IO_STATUS_BLOCK};
    use std::io::{Error, ErrorKind};
    use std::os::windows::io::AsRawHandle;
    use winapi::shared::ntdef::NT_SUCCESS;
    use winapi::um::winioctl::FSCTL_GET_REPARSE_POINT;
    use winapi::um::winnt::{IO_REPARSE_TAG_SYMLINK, MAXIMUM_REPARSE_DATA_BUFFER_SIZE};

    let mut buffer = vec![0u8; MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize];
    // SAFETY: IO_STATUS_BLOCK is a plain C data structure initialized before the synchronous call.
    let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: the synchronous call receives a valid handle and a writable output buffer.
    let status = unsafe {
        NtFsControlFile(
            file.as_raw_handle().cast(),
            std::ptr::null_mut(),
            None,
            std::ptr::null_mut(),
            &mut io_status,
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null_mut(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if !NT_SUCCESS(status) {
        return Err(windows_nt_status_error(status));
    }
    let returned = io_status.Information;
    if returned < 20 || returned > buffer.len() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid untracked reparse point data",
        ));
    }
    let buffer = &buffer[..returned];
    let tag = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
    if tag != IO_REPARSE_TAG_SYMLINK {
        return Ok(None);
    }
    let data_length = u16::from_le_bytes(buffer[4..6].try_into().unwrap()) as usize;
    let total_length = 8usize
        .checked_add(data_length)
        .filter(|length| *length <= buffer.len())
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid untracked symlink data"))?;
    let substitute_offset = u16::from_le_bytes(buffer[8..10].try_into().unwrap()) as usize;
    let substitute_length = u16::from_le_bytes(buffer[10..12].try_into().unwrap()) as usize;
    let print_offset = u16::from_le_bytes(buffer[12..14].try_into().unwrap()) as usize;
    let print_length = u16::from_le_bytes(buffer[14..16].try_into().unwrap()) as usize;
    let (offset, length) = if print_length == 0 {
        (substitute_offset, substitute_length)
    } else {
        (print_offset, print_length)
    };
    if length % 2 != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid untracked symlink target",
        ));
    }
    let start = 20usize
        .checked_add(offset)
        .filter(|start| *start <= total_length)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid untracked symlink target"))?;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= total_length)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid untracked symlink target"))?;
    let target: Vec<u16> = buffer[start..end]
        .chunks_exact(2)
        .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
        .collect();
    String::from_utf16(&target).map(Some).map_err(|_| {
        Error::new(
            ErrorKind::InvalidData,
            "untracked symlink target is not UTF-16",
        )
    })
}

#[cfg(windows)]
fn windows_nt_status_error(status: winapi::shared::ntdef::NTSTATUS) -> std::io::Error {
    // SAFETY: RtlNtStatusToDosError accepts every NTSTATUS value.
    let error = unsafe { ntapi::ntrtl::RtlNtStatusToDosError(status) };
    std::io::Error::from_raw_os_error(error as i32)
}

#[cfg(not(any(unix, windows)))]
fn read_untracked_content(
    _repo_root: &UntrackedRoot,
    path: &Path,
) -> std::io::Result<Option<(&'static str, String)>> {
    validated_relative_components(path)?;
    Ok(None)
}

/// Synthesize a unified `new file` diff for each untracked path so
/// downstream parsers and the review prompt can treat them as
/// additions. Symlinks are represented by their link text, matching Git.
/// Binary or unreadable files are skipped.
fn synthesize_untracked_diff(repo_root: &UntrackedRoot, paths: &[String]) -> Result<String> {
    let mut out = String::new();
    for path in paths {
        let Some((mode, content)) = (match read_untracked_content(repo_root, Path::new(path)) {
            Ok(content) => content,
            Err(_) => continue,
        }) else {
            continue;
        };
        out.push_str(&format!("diff --git a/{path} b/{path}\n"));
        out.push_str(&format!("new file mode {mode}\n"));
        out.push_str("--- /dev/null\n");
        out.push_str(&format!("+++ b/{path}\n"));
        let trailing_newline = content.ends_with('\n');
        let line_count = if content.is_empty() {
            0
        } else if trailing_newline {
            content.matches('\n').count()
        } else {
            content.matches('\n').count() + 1
        };
        if line_count > 0 {
            out.push_str(&format!("@@ -0,0 +1,{line_count} @@\n"));
            for line in content.split_inclusive('\n') {
                let body = line.strip_suffix('\n').unwrap_or(line);
                out.push('+');
                out.push_str(body);
                out.push('\n');
            }
            if !trailing_newline {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    Ok(out)
}

/// Convert repo-relative `touched` paths into paths relative to
/// `discovery_root` so [`goose::checks::discover`] doesn't double-
/// prefix `<scope>/api/...` when `--check-scope` points at a subtree.
/// Files outside the scope are dropped — they cannot affect any
/// scoped check inside `discovery_root`.
fn rebase_touched_to_scope(
    repo_root: &Path,
    discovery_root: &Path,
    touched: &[String],
) -> Vec<String> {
    if discovery_root == repo_root {
        return touched.to_vec();
    }
    let prefix = match discovery_root.strip_prefix(repo_root) {
        Ok(p) => p,
        Err(_) => return touched.to_vec(),
    };
    let prefix_str = prefix.to_string_lossy().replace('\\', "/");
    if prefix_str.is_empty() {
        return touched.to_vec();
    }
    let prefix_with_slash = format!("{prefix_str}/");
    touched
        .iter()
        .filter_map(|p| p.strip_prefix(&prefix_with_slash).map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use goose::checks::Check;
    use std::path::PathBuf;

    #[cfg(unix)]
    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn review_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run_git(root, &["init", "--quiet"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test"]);
        fs::write(root.join("tracked.txt"), "before\n").unwrap();
        run_git(root, &["add", "tracked.txt"]);
        run_git(
            root,
            &["commit", "--quiet", "--no-gpg-sign", "-m", "initial"],
        );
        dir
    }

    #[cfg(unix)]
    fn write_executable_script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, body).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn open_test_untracked_root(path: &Path) -> std::io::Result<UntrackedRoot> {
        #[cfg(unix)]
        let path = fs::canonicalize(path)?;
        #[cfg(not(unix))]
        let path = path.to_path_buf();
        open_untracked_root(&path)
    }

    fn ck(name: &str) -> Check {
        Check {
            name: name.to_string(),
            description: None,
            model: None,
            turn_limit: None,
            tools: None,
            severity_default: None,
            path: PathBuf::from(format!("/.agents/checks/{name}.md")),
            scope_dir: String::new(),
            body: "body".into(),
        }
    }

    #[test]
    fn filter_checks_passes_through_when_filter_empty() {
        let d = DiscoveredReview {
            checks: vec![ck("perf"), ck("security")],
        };
        let out = filter_checks(d, &[]);
        assert_eq!(out.checks.len(), 2);
    }

    #[test]
    fn filter_checks_keeps_only_named_checks() {
        let d = DiscoveredReview {
            checks: vec![ck("perf"), ck("security"), ck("idempotency")],
        };
        let out = filter_checks(d, &["security".to_string(), "idempotency".to_string()]);
        let names: Vec<&str> = out.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["security", "idempotency"]);
    }

    #[test]
    fn legacy_review_allows_checks_without_tool_policy() {
        let discovered = DiscoveredReview {
            checks: vec![ck("security")],
        };

        ensure_legacy_check_tools_are_unrestricted(&discovered).unwrap();
    }

    #[test]
    fn legacy_review_rejects_nonempty_tool_allowlists() {
        let mut restricted = ck("security");
        restricted.tools = Some(vec!["read".to_string()]);
        let discovered = DiscoveredReview {
            checks: vec![restricted],
        };

        let error = ensure_legacy_check_tools_are_unrestricted(&discovered).unwrap_err();
        assert!(error.to_string().contains("security"));
        assert!(error.to_string().contains("without --no-orchestrate"));
    }

    #[test]
    fn legacy_review_rejects_explicit_empty_tool_allowlists() {
        let mut restricted = ck("no-tools");
        restricted.tools = Some(Vec::new());
        let discovered = DiscoveredReview {
            checks: vec![restricted],
        };

        let error = ensure_legacy_check_tools_are_unrestricted(&discovered).unwrap_err();
        assert!(error.to_string().contains("no-tools"));
    }

    #[test]
    fn prepend_instructions_noop_when_none_or_empty() {
        assert_eq!(prepend_instructions("BASE", None), "BASE");
        assert_eq!(prepend_instructions("BASE", Some("   ")), "BASE");
    }

    #[test]
    fn prepend_instructions_adds_block_above_base() {
        let out = prepend_instructions("BASE", Some("Refactor only — flag any behavior change."));
        assert!(out.starts_with("## Reviewer instructions\n\nRefactor only"));
        assert!(out.ends_with("BASE"));
    }

    #[cfg(unix)]
    #[test]
    fn review_diff_collection_ignores_external_diff_helpers() {
        let dir = review_test_repo();
        let root = dir.path();
        let marker = root.join("external-diff-ran");
        let script = root.join("external-diff.sh");
        write_executable_script(
            &script,
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/external-diff-ran\"\nexit 0\n",
        );
        run_git(root, &["config", "diff.external", script.to_str().unwrap()]);
        fs::write(root.join("tracked.txt"), "after\n").unwrap();

        assert_eq!(touched_files(root, None, &[]).unwrap(), ["tracked.txt"]);
        let diff = collect_diff(root, None, &[]).unwrap();
        assert!(diff.contains("-before"));
        assert!(diff.contains("+after"));
        assert!(collect_diff_stat(root, None, &[])
            .unwrap()
            .contains("tracked.txt"));
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn review_diff_collection_ignores_textconv_helpers() {
        let dir = review_test_repo();
        let root = dir.path();
        let marker = root.join("textconv-ran");
        let script = root.join("textconv.sh");
        write_executable_script(
            &script,
            "#!/bin/sh\ntouch \"$(dirname \"$0\")/textconv-ran\"\ncat \"$1\"\n",
        );
        fs::write(
            root.join(".gitattributes"),
            "tracked.txt diff=review-test\n",
        )
        .unwrap();
        run_git(root, &["add", ".gitattributes"]);
        run_git(
            root,
            &["commit", "--quiet", "--no-gpg-sign", "-m", "add attributes"],
        );
        run_git(
            root,
            &[
                "config",
                "diff.review-test.textconv",
                script.to_str().unwrap(),
            ],
        );
        fs::write(root.join("tracked.txt"), "after\n").unwrap();

        let diff = collect_diff(root, None, &[]).unwrap();
        assert!(diff.contains("-before"));
        assert!(diff.contains("+after"));
        assert!(!marker.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn synthesize_untracked_diff_emits_new_file_chunk_with_added_lines() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("new/file.txt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

        let root = open_test_untracked_root(root).unwrap();
        let diff = synthesize_untracked_diff(&root, &["new/file.txt".to_string()]).unwrap();
        assert!(diff.contains("diff --git a/new/file.txt b/new/file.txt"));
        assert!(diff.contains("new file mode 100644"));
        assert!(diff.contains("--- /dev/null"));
        assert!(diff.contains("+++ b/new/file.txt"));
        assert!(diff.contains("@@ -0,0 +1,3 @@"));
        assert!(diff.contains("+alpha\n+beta\n+gamma\n"));
        assert!(!diff.contains("\\ No newline at end of file"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn synthesize_untracked_diff_marks_missing_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "no-newline").unwrap();

        let root = open_test_untracked_root(root).unwrap();
        let diff = synthesize_untracked_diff(&root, &["a.txt".to_string()]).unwrap();
        assert!(diff.contains("@@ -0,0 +1,1 @@"));
        assert!(diff.contains("+no-newline\n"));
        assert!(diff.contains("\\ No newline at end of file"));
    }

    #[cfg(unix)]
    #[test]
    fn synthesize_untracked_diff_uses_symlink_text_without_following_target() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "TOPSECRET-OUTSIDE-REPO").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("link.txt")).unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let diff = synthesize_untracked_diff(&root, &["link.txt".to_string()]).unwrap();

        assert!(diff.contains("new file mode 120000"));
        assert!(diff.contains(&format!("+{}", secret.display())));
        assert!(!diff.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(unix)]
    #[test]
    fn synthesize_untracked_diff_includes_broken_symlink_text() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("../missing-target", dir.path().join("broken")).unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let diff = synthesize_untracked_diff(&root, &["broken".to_string()]).unwrap();

        assert!(diff.contains("new file mode 120000"));
        assert!(diff.contains("+../missing-target"));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_file_reader_preserves_link_text_after_leaf_swap() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = dir.path().join("untracked.txt");
        let secret = outside.path().join("secret.txt");
        fs::write(&path, "safe worktree content").unwrap();
        fs::write(&secret, "TOPSECRET-OUTSIDE-REPO").unwrap();

        assert!(fs::symlink_metadata(&path).unwrap().is_file());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let (mode, content) = read_untracked_content(&root, Path::new("untracked.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(mode, "120000");
        assert_eq!(content, secret.to_str().unwrap());
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_file_reader_stays_in_opened_ancestor_after_swap() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ancestor = dir.path().join("nested");
        let moved_ancestor = dir.path().join("moved-nested");
        fs::create_dir(&ancestor).unwrap();
        fs::write(ancestor.join("file.txt"), "safe worktree content").unwrap();
        fs::write(outside.path().join("file.txt"), "TOPSECRET-OUTSIDE-REPO").unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let (mode, content) =
            read_untracked_content_with_hook(&root, Path::new("nested/file.txt"), |opened_path| {
                if opened_path == Path::new("nested") {
                    fs::rename(&ancestor, &moved_ancestor).unwrap();
                    std::os::unix::fs::symlink(outside.path(), &ancestor).unwrap();
                }
            })
            .unwrap()
            .unwrap();

        assert_eq!(mode, "100644");
        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_file_reader_stays_in_opened_root_after_swap() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root_path = parent.path().join("repo");
        let moved_root = parent.path().join("moved-repo");
        fs::create_dir(&root_path).unwrap();
        fs::write(root_path.join("file.txt"), "safe worktree content").unwrap();
        fs::write(outside.path().join("file.txt"), "TOPSECRET-OUTSIDE-REPO").unwrap();
        let root = open_test_untracked_root(&root_path).unwrap();

        fs::rename(&root_path, &moved_root).unwrap();
        std::os::unix::fs::symlink(outside.path(), &root_path).unwrap();

        let (mode, content) = read_untracked_content(&root, Path::new("file.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(mode, "100644");
        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_enumeration_stays_in_opened_root_after_swap() {
        let parent = tempfile::tempdir().unwrap();
        let root_path = parent.path().join("repo");
        let moved_root = parent.path().join("moved-repo");
        fs::create_dir(&root_path).unwrap();
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root_path)
            .status()
            .unwrap()
            .success());
        fs::write(root_path.join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(root_path.join("secret.txt"), "original ignored content").unwrap();
        let root = open_test_untracked_root(&root_path).unwrap();

        fs::rename(&root_path, &moved_root).unwrap();
        fs::create_dir(&root_path).unwrap();
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root_path)
            .status()
            .unwrap()
            .success());
        fs::write(
            root_path.join("secret.txt"),
            "replacement untracked content",
        )
        .unwrap();

        let untracked = untracked_files(&root, &[]).unwrap();
        assert!(untracked.contains(&".gitignore".to_string()));
        assert!(!untracked.contains(&"secret.txt".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_root_rejects_symlinked_ancestor() {
        let parent = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(parent.path()).unwrap();
        let real_parent = parent.join("real-parent");
        let linked_parent = parent.join("linked-parent");
        fs::create_dir(&real_parent).unwrap();
        std::os::unix::fs::symlink(&real_parent, &linked_parent).unwrap();
        fs::create_dir(linked_parent.join("repo")).unwrap();

        let error = match open_untracked_root(&linked_parent.join("repo")) {
            Ok(_) => panic!("symlinked ancestor was accepted"),
            Err(error) => error,
        };

        assert!(matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::ENOTDIR)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn untracked_root_stays_in_opened_ancestor_after_swap() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = fs::canonicalize(parent.path()).unwrap();
        let ancestor = parent.join("ancestor");
        let moved_ancestor = parent.join("moved-ancestor");
        let root_path = ancestor.join("repo");
        fs::create_dir_all(&root_path).unwrap();
        fs::write(root_path.join("file.txt"), "safe worktree content").unwrap();
        fs::create_dir(outside.path().join("repo")).unwrap();
        fs::write(
            outside.path().join("repo/file.txt"),
            "TOPSECRET-OUTSIDE-REPO",
        )
        .unwrap();

        let root = open_untracked_root_with_hook(&root_path, |opened_path| {
            if opened_path == ancestor {
                fs::rename(&ancestor, &moved_ancestor).unwrap();
                std::os::unix::fs::symlink(outside.path(), &ancestor).unwrap();
            }
        })
        .unwrap();
        let (_, content) = read_untracked_content(&root, Path::new("file.txt"))
            .unwrap()
            .unwrap();

        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_untracked_file_reader_stays_in_opened_ancestor_after_swap() {
        let dir = tempfile::tempdir().unwrap();
        let ancestor = dir.path().join("nested");
        let moved_ancestor = dir.path().join("moved-nested");
        fs::create_dir(&ancestor).unwrap();
        fs::write(ancestor.join("file.txt"), "safe worktree content").unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let (mode, content) =
            read_untracked_content_with_hook(&root, Path::new("nested/file.txt"), |opened_path| {
                if opened_path == Path::new("nested") {
                    fs::rename(&ancestor, &moved_ancestor).unwrap();
                    fs::create_dir(&ancestor).unwrap();
                    fs::write(ancestor.join("file.txt"), "TOPSECRET-OUTSIDE-REPO").unwrap();
                }
            })
            .unwrap()
            .unwrap();

        assert_eq!(mode, "100644");
        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_untracked_root_prevents_swap() {
        let parent = tempfile::tempdir().unwrap();
        let root_path = parent.path().join("repo");
        let moved_root = parent.path().join("moved-repo");
        fs::create_dir(&root_path).unwrap();
        fs::write(root_path.join("file.txt"), "safe worktree content").unwrap();
        let root = open_test_untracked_root(&root_path).unwrap();

        assert!(fs::rename(&root_path, &moved_root).is_err());

        let (mode, content) = read_untracked_content(&root, Path::new("file.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(mode, "100644");
        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_untracked_root_prevents_ancestor_reparse_swap() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ancestor = parent.path().join("ancestor");
        let moved_ancestor = parent.path().join("moved-ancestor");
        let replacement = parent.path().join("replacement");
        let root_path = ancestor.join("repo");
        fs::create_dir_all(&root_path).unwrap();
        fs::write(root_path.join("file.txt"), "safe worktree content").unwrap();
        fs::create_dir(outside.path().join("repo")).unwrap();
        fs::write(
            outside.path().join("repo/file.txt"),
            "TOPSECRET-OUTSIDE-REPO",
        )
        .unwrap();
        if std::os::windows::fs::symlink_dir(outside.path(), &replacement).is_err() {
            return;
        }

        let root = open_untracked_root_with_hook(&root_path, |opened_path| {
            if opened_path == ancestor {
                assert!(fs::rename(&ancestor, &moved_ancestor).is_err());
            }
        })
        .unwrap();
        let (_, content) = read_untracked_content(&root, Path::new("file.txt"))
            .unwrap()
            .unwrap();

        assert_eq!(content, "safe worktree content");
        assert!(!content.contains("TOPSECRET-OUTSIDE-REPO"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_untracked_file_reader_rejects_reparse_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("file.txt"), "TOPSECRET-OUTSIDE-REPO").unwrap();
        if std::os::windows::fs::symlink_dir(outside.path(), dir.path().join("nested")).is_err() {
            return;
        }

        let root = open_test_untracked_root(dir.path()).unwrap();
        let result = read_untracked_content(&root, Path::new("nested/file.txt"));

        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_synthesize_untracked_diff_preserves_symlink_text() {
        let dir = tempfile::tempdir().unwrap();
        let target = Path::new("missing-target.txt");
        if std::os::windows::fs::symlink_file(target, dir.path().join("link.txt")).is_err() {
            return;
        }

        let root = open_test_untracked_root(dir.path()).unwrap();
        let diff = synthesize_untracked_diff(&root, &["link.txt".to_string()]).unwrap();

        assert!(diff.contains("new file mode 120000"));
        assert!(diff.contains("+missing-target.txt"));
    }

    #[test]
    fn untracked_paths_must_be_repo_relative() {
        for path in [
            Path::new("/outside"),
            Path::new("../outside"),
            Path::new("nested/../../outside"),
        ] {
            let error = validated_relative_components(path).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
        assert_eq!(
            validated_relative_components(Path::new("nested/./file.txt")).unwrap(),
            [
                std::ffi::OsStr::new("nested"),
                std::ffi::OsStr::new("file.txt")
            ]
        );

        #[cfg(windows)]
        assert_eq!(
            validated_relative_components(Path::new(r"C:\outside"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[cfg(not(any(unix, windows)))]
    #[test]
    fn synthesize_untracked_diff_omits_ordinary_files_without_safe_open() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ordinary.txt"), "ordinary content").unwrap();

        let root = open_test_untracked_root(dir.path()).unwrap();
        let diff = synthesize_untracked_diff(&root, &["ordinary.txt".to_string()]).unwrap();

        assert!(diff.is_empty());
    }

    #[test]
    fn rebase_touched_to_scope_strips_scope_prefix() {
        let repo = PathBuf::from("/repo");
        let scope = PathBuf::from("/repo/api/v2");
        let touched = vec![
            "api/v2/foo.rs".to_string(),
            "api/v2/bar.rs".to_string(),
            "frontend/main.tsx".to_string(),
        ];
        let out = rebase_touched_to_scope(&repo, &scope, &touched);
        assert_eq!(out, vec!["foo.rs", "bar.rs"]);
    }

    #[test]
    fn rebase_touched_to_scope_passes_through_when_scope_equals_repo() {
        let repo = PathBuf::from("/repo");
        let touched = vec!["a.rs".to_string()];
        let out = rebase_touched_to_scope(&repo, &repo, &touched);
        assert_eq!(out, touched);
    }
}
