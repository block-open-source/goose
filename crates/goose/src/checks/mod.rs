//! Discovery and parsing for review checks (`.agents/checks/*.md` and
//! `**/.agents/REVIEW.md`). Reuses the frontmatter parser exported by
//! [`crate::sources`] so all source-style files share one YAML pipeline.
//!
//! User-facing CRUD lives in `crate::sources` for parity with skills and
//! projects; `goose review` consumes [`Check`] and [`discover`] directly.

use crate::sources::parse_frontmatter;
use anyhow::{anyhow, bail, Context, Result};
use goose_sdk_types::custom_requests::{SourceEntry, SourceType};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Default maximum number of turns a check subagent may take.
///
/// Mirrors `goose::agents::subagent_task_config::DEFAULT_SUBAGENT_MAX_TURNS`,
/// duplicated here to keep the checks module self-contained for parsing.
pub const DEFAULT_CHECK_TURN_LIMIT: usize = 25;

/// Virtual check name prefix for `REVIEW.md`-derived checks. Findings emitted
/// by these checks should be attributed to the originating `REVIEW.md`.
pub const REVIEW_MD_CHECK_PREFIX: &str = "repo-rules";

/// Parsed YAML frontmatter for a check file.
#[derive(Debug, Deserialize, Default)]
struct CheckFrontmatter {
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "turn-limit")]
    turn_limit: Option<usize>,
    tools: Option<Vec<String>>,
    #[serde(rename = "severity-default")]
    severity_default: Option<String>,
}

/// A parsed check definition from `**/.agents/checks/*.md`.
///
/// Each check is a Markdown file with YAML frontmatter and a body of
/// natural-language instructions for the subagent reviewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Identifier for the check. Must match the file's basename (without `.md`)
    /// for repo-local checks; globals are allowed to drift for cross-tool
    /// compatibility.
    pub name: String,
    /// Brief description shown when listing checks.
    pub description: Option<String>,
    /// Per-check model override. Resolved against CLI flags by [`Check::resolved_model`].
    pub model: Option<String>,
    /// Per-check turn limit. Resolved against the global default by
    /// [`Check::resolved_turn_limit`].
    pub turn_limit: Option<usize>,
    /// Optional allowlist of tool names the check subagent may call.
    /// `None` (the default) means the subagent inherits the agent's full
    /// toolset. Mirrors Amp's `tools:` field for parity with
    /// `.agents/checks/*.md`.
    pub tools: Option<Vec<String>>,
    /// Optional default severity for findings emitted by this check
    /// (`low`, `medium`, `high`, `critical`). Recognized for parity with
    /// Amp's `severity-default:` field.
    pub severity_default: Option<String>,
    /// Absolute path to the check file on disk.
    pub path: PathBuf,
    /// Repo-relative scope directory the check applies to. Empty string means
    /// the repo root or a global location.
    pub scope_dir: String,
    /// Markdown content after the closing `---`.
    pub body: String,
}

impl Check {
    /// Read and parse a check file from disk.
    pub fn from_path(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("check {}: invalid path", path.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow!("check {}: invalid filename", path.display()))?;
        let content = crate::skills::read_source_file(parent, Path::new(file_name))
            .with_context(|| format!("read check file: {}", path.display()))?;
        Self::parse(&content, path)
    }

    /// Parse a check from raw content. Reuses [`crate::sources::parse_frontmatter`]
    /// so checks share the same YAML pipeline as skills and projects.
    pub fn parse(content: &str, path: &Path) -> Result<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            bail!(
                "check {}: missing frontmatter (must start with ---)",
                path.display()
            );
        }

        // Tolerate CRLF line endings before delegating to the shared splitter.
        let normalized = trimmed.replace("\r\n", "\n");
        let (frontmatter, body) = match parse_frontmatter::<CheckFrontmatter>(&normalized) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => bail!(
                "check {}: missing closing --- in frontmatter",
                path.display()
            ),
            Err(e) => {
                return Err(anyhow!(e))
                    .with_context(|| format!("check {}: invalid frontmatter YAML", path.display()))
            }
        };

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("check {}: invalid filename", path.display()))?
            .to_string();

        let name = match frontmatter.name {
            Some(declared) if !declared.is_empty() => declared,
            _ => file_stem,
        };

        Ok(Check {
            name,
            description: frontmatter.description,
            model: frontmatter.model,
            turn_limit: frontmatter.turn_limit,
            tools: frontmatter.tools,
            severity_default: frontmatter.severity_default,
            path: path.to_path_buf(),
            scope_dir: String::new(),
            body,
        })
    }

    /// Verify the check's `name` matches its filename stem (e.g. `perf` for
    /// `perf.md`). Repo-local checks are required to satisfy this rule so
    /// authors get a clear error early; checks loaded from global directories
    /// are allowed to drift for compatibility with cross-tool conventions.
    pub fn validate_name_matches_filename(&self) -> Result<()> {
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("check {}: invalid filename", self.path.display()))?;
        if self.name != stem {
            bail!(
                "check {}: name '{}' must match filename '{}'",
                self.path.display(),
                self.name,
                stem
            );
        }
        Ok(())
    }

    /// Resolve which model this check should use.
    ///
    /// `override_model` (CLI `--override-model`) wins over everything; otherwise
    /// the per-check `model` wins; otherwise `default_model` (CLI `--model`).
    pub fn resolved_model<'a>(
        &'a self,
        default_model: Option<&'a str>,
        override_model: Option<&'a str>,
    ) -> Option<&'a str> {
        if let Some(m) = override_model {
            return Some(m);
        }
        if let Some(m) = self.model.as_deref() {
            return Some(m);
        }
        default_model
    }

    /// Resolve the turn limit for this check.
    pub fn resolved_turn_limit(&self, default_turn_limit: Option<usize>) -> usize {
        self.turn_limit
            .or(default_turn_limit)
            .unwrap_or(DEFAULT_CHECK_TURN_LIMIT)
    }

    /// Render this check as a generic [`SourceEntry`] so it can flow through
    /// the same listing/UI pipeline as skills and projects. Checks surface as
    /// [`SourceType::Agent`] entries — they're sub-agent definitions
    /// specialized for code review — with `properties["kind"] = "check"`
    /// so clients can distinguish them from `.agents/agents/*.md` agents.
    /// Per-check tunables (`model`, `turn-limit`, `tools`, `severity-default`,
    /// `scope_dir`) live in `properties` so the SDK schema doesn't need
    /// check-specific fields.
    pub fn to_source_entry(&self, global: bool) -> SourceEntry {
        let mut properties: HashMap<String, serde_json::Value> = HashMap::new();
        properties.insert("kind".into(), serde_json::Value::String("check".into()));
        if let Some(m) = &self.model {
            properties.insert("model".into(), serde_json::Value::String(m.clone()));
        }
        if let Some(n) = self.turn_limit {
            properties.insert("turnLimit".into(), serde_json::Value::from(n));
        }
        if let Some(t) = &self.tools {
            properties.insert(
                "tools".into(),
                serde_json::Value::Array(
                    t.iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(sev) = &self.severity_default {
            properties.insert(
                "severityDefault".into(),
                serde_json::Value::String(sev.clone()),
            );
        }
        if !self.scope_dir.is_empty() {
            properties.insert(
                "scopeDir".into(),
                serde_json::Value::String(self.scope_dir.clone()),
            );
        }
        SourceEntry {
            source_type: SourceType::Agent,
            name: self.name.clone(),
            description: self.description.clone().unwrap_or_default(),
            content: self.body.clone(),
            path: self.path.to_string_lossy().into_owned(),
            global,
            writable: true,
            supporting_files: Vec::new(),
            properties,
        }
    }
}

/// Discovered set of checks for a review request.
///
/// `checks` contains both author-defined checks (from `.agents/checks/*.md`)
/// and virtual checks auto-derived from `**/.agents/REVIEW.md` files so their
/// findings can be attributed back to that file.
#[derive(Debug, Default, Clone)]
pub struct DiscoveredReview {
    /// All applicable checks, sorted by name. Closer scopes shadow same-named
    /// checks from broader scopes (project root, then home/global).
    pub checks: Vec<Check>,
}

fn review_md_virtual_check_name(scope_dir: &str) -> String {
    if scope_dir.is_empty() {
        REVIEW_MD_CHECK_PREFIX.to_string()
    } else {
        format!("{REVIEW_MD_CHECK_PREFIX}:{scope_dir}")
    }
}

fn synthesize_review_md_check(scope_dir: &str, path: &Path, body: &str) -> Check {
    let scope_label = if scope_dir.is_empty() {
        "the entire repository".to_string()
    } else {
        format!("files under `{scope_dir}/`")
    };
    let intro = format!(
        "You are enforcing the project's `REVIEW.md` rules for {scope_label}.\n\n\
         These rules were authored in `{}`. Apply them strictly to the diff.\n\n\
         ---\n\n",
        path.display(),
    );
    Check {
        name: review_md_virtual_check_name(scope_dir),
        description: Some(format!("Auto-derived from {}", path.display())),
        model: None,
        turn_limit: None,
        tools: None,
        severity_default: None,
        path: path.to_path_buf(),
        scope_dir: scope_dir.to_string(),
        body: format!("{intro}{}", body.trim()),
    }
}

fn read_review_md(path: &Path, canonical_repo_root: &Path) -> Result<Option<String>> {
    let canonical_path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };

    if !canonical_path.starts_with(canonical_repo_root) || !canonical_path.is_file() {
        return Ok(None);
    }

    fs::read_to_string(&canonical_path)
        .map(Some)
        .with_context(|| format!("read REVIEW.md {}", path.display()))
}

/// Locations searched for global checks, in priority order.
///
/// The first existing directory wins for a given check name; closer scopes
/// (repo root, then sub-trees) shadow these globals when names collide.
pub fn global_checks_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs_home() {
        dirs.push(home.join(".config").join("goose").join("checks"));
        dirs.push(home.join(".config").join("agents").join("checks"));
    }
    dirs
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// Discover all checks and REVIEW.md files relevant to `touched_files`.
///
/// `repo_root` is the absolute path to the repository root; `touched_files` is
/// the list of repo-relative file paths changed in the diff. When
/// `touched_files` is empty, only repo-root and global locations are
/// considered.
pub fn discover(repo_root: &Path, touched_files: &[String]) -> Result<DiscoveredReview> {
    discover_with_globals(repo_root, touched_files, &global_checks_dirs())
}

/// Discover checks against an explicit set of global directories. Exposed for
/// tests and for callers that want to override the global search path.
pub fn discover_with_globals(
    repo_root: &Path,
    touched_files: &[String],
    global_dirs: &[PathBuf],
) -> Result<DiscoveredReview> {
    let scope_dirs = candidate_scope_dirs(touched_files);
    let canonical_repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize repo root {}", repo_root.display()))?;

    // Collect raw checks keyed by name with explicit per-source priority so
    // closer/more-specific sources shadow broader ones. Globals get priority
    // 0 so a repo-root check (priority 1) of the same name always wins.
    let mut by_name: BTreeMap<String, (usize, Check)> = BTreeMap::new();
    let mut record = |check: Check, priority: usize| {
        by_name
            .entry(check.name.clone())
            .and_modify(|(existing_priority, existing)| {
                if priority > *existing_priority {
                    *existing = check.clone();
                    *existing_priority = priority;
                }
            })
            .or_insert((priority, check));
    };

    for dir in global_dirs {
        let Ok(canonical_dir) = dir.canonicalize() else {
            continue;
        };
        for mut check in read_checks_dir(&canonical_dir, &canonical_dir, "", LoadMode::Lenient)? {
            if let Some(file_name) = check.path.file_name().map(ToOwned::to_owned) {
                check.path = dir.join(file_name);
            }
            record(check, 0);
        }
    }

    let root_dir = canonical_repo_root.join(".agents").join("checks");
    for check in read_checks_dir(&canonical_repo_root, &root_dir, "", LoadMode::Strict)? {
        record(check, scope_priority(""));
    }

    for scope in &scope_dirs {
        let dir = canonical_repo_root
            .join(scope)
            .join(".agents")
            .join("checks");
        for check in read_checks_dir(&canonical_repo_root, &dir, scope, LoadMode::Strict)? {
            let p = scope_priority(scope);
            record(check, p);
        }
    }

    let root_review = repo_root.join(".agents").join("REVIEW.md");
    if let Some(body) = read_review_md(&root_review, &canonical_repo_root)? {
        let check = synthesize_review_md_check("", &root_review, &body);
        record(check, scope_priority(""));
    }
    for scope in &scope_dirs {
        let path = repo_root.join(scope).join(".agents").join("REVIEW.md");
        if let Some(body) = read_review_md(&path, &canonical_repo_root)? {
            let check = synthesize_review_md_check(scope, &path, &body);
            let p = scope_priority(scope);
            record(check, p);
        }
    }

    let mut checks: Vec<Check> = by_name.into_values().map(|(_, c)| c).collect();
    checks.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(DiscoveredReview { checks })
}

/// Whether parse errors in a check directory should fail the run or be skipped
/// with a warning. Repo-local checks are loaded `Strict` because authors should
/// see their own broken frontmatter; global directories are loaded `Lenient`
/// because they often contain `README.md` and similar non-check Markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadMode {
    Strict,
    Lenient,
}

fn read_checks_dir(
    scan_root: &Path,
    dir: &Path,
    scope_dir: &str,
    mode: LoadMode,
) -> Result<Vec<Check>> {
    if !checks_dir_is_unlinked(scan_root, dir)? {
        return Ok(Vec::new());
    }
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(Vec::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect checks dir {}", dir.display()))
        }
    }
    let mut out = Vec::new();
    let entries =
        fs::read_dir(dir).with_context(|| format!("read checks dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        // README.md is conventionally the directory's docs, not a check.
        if path.file_name().and_then(|s| s.to_str()) == Some("README.md") {
            continue;
        }
        let parsed = Check::from_path(&path).and_then(|mut check| {
            if mode == LoadMode::Strict {
                check.validate_name_matches_filename()?;
            }
            check.scope_dir = scope_dir.to_string();
            Ok(check)
        });
        match parsed {
            Ok(check) => out.push(check),
            Err(e) => match mode {
                LoadMode::Strict => return Err(e),
                LoadMode::Lenient => {
                    eprintln!("goose review: skipping {}: {e}", path.display());
                }
            },
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn checks_dir_is_unlinked(scan_root: &Path, dir: &Path) -> Result<bool> {
    let relative = dir
        .strip_prefix(scan_root)
        .with_context(|| format!("checks dir {} is outside scan root", dir.display()))?;
    let mut current = scan_root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Ok(false);
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(false),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect checks dir component {}", current.display()))
            }
        }
    }
    Ok(true)
}

/// Priority of a scope for shadowing checks/overrides. Higher = closer.
fn scope_priority(scope_dir: &str) -> usize {
    if scope_dir.is_empty() {
        1
    } else {
        2 + scope_dir.split('/').count()
    }
}

/// Compute the set of in-repo scope directories whose `.agents/` may contain
/// checks or REVIEW.md applicable to the touched files.
///
/// For `api/v2/foo.rs` this yields `["api", "api/v2"]`.
pub fn candidate_scope_dirs(touched_files: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for file in touched_files {
        let normalized = file.replace('\\', "/");
        let dir = Path::new(&normalized).parent();
        let Some(dir) = dir else { continue };
        let parts: Vec<&str> = dir
            .to_str()
            .unwrap_or_default()
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        for i in 1..=parts.len() {
            seen.insert(parts[..i].join("/"));
        }
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn parses_full_frontmatter() {
        let content = r#"---
name: perf
description: Flag perf regressions
model: claude-sonnet-4
turn-limit: 40
tools:
  - read
  - grep
---
Look for N+1 queries.
"#;
        let check = Check::parse(content, &p("/r/.agents/checks/perf.md")).unwrap();
        assert_eq!(check.name, "perf");
        assert_eq!(check.description.as_deref(), Some("Flag perf regressions"));
        assert_eq!(check.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(check.turn_limit, Some(40));
        assert_eq!(
            check.tools.as_deref(),
            Some(["read".to_string(), "grep".to_string()].as_slice())
        );
        assert_eq!(check.body, "Look for N+1 queries.");
    }

    #[test]
    fn tools_field_is_optional_for_backward_compatibility() {
        let content = "---\nname: legacy\n---\nbody";
        let check = Check::parse(content, &p("/r/.agents/checks/legacy.md")).unwrap();
        assert!(check.tools.is_none());
        assert!(check.severity_default.is_none());
    }

    #[test]
    fn parses_extended_frontmatter() {
        let content = r#"---
name: untrusted-pr
description: Reviews PRs as untrusted input
severity-default: high
tools: [Bash, Read, Grep]
---

## Purpose
"#;
        let check = Check::parse(content, &p("/r/.agents/checks/untrusted-pr.md")).unwrap();
        assert_eq!(check.name, "untrusted-pr");
        assert_eq!(check.severity_default.as_deref(), Some("high"));
        assert_eq!(
            check.tools.as_deref(),
            Some(["Bash".to_string(), "Read".to_string(), "Grep".to_string()].as_slice())
        );
        assert!(check.model.is_none());
        assert!(check.turn_limit.is_none());
    }

    #[test]
    fn defaults_name_to_filename_stem() {
        let content = "---\ndescription: foo\n---\nbody";
        let check = Check::parse(content, &p("/r/.agents/checks/sql-safety.md")).unwrap();
        assert_eq!(check.name, "sql-safety");
    }

    #[test]
    fn parse_keeps_declared_name_for_global_compat() {
        let content = "---\nname: meta-review\n---\nbody";
        let check = Check::parse(content, &p("/global/checks/review.md")).unwrap();
        assert_eq!(check.name, "meta-review");
    }

    #[test]
    fn validate_name_matches_filename_rejects_mismatch() {
        let content = "---\nname: other\n---\nbody";
        let check = Check::parse(content, &p("/r/.agents/checks/perf.md")).unwrap();
        let err = check.validate_name_matches_filename().unwrap_err();
        assert!(err.to_string().contains("must match filename"));
    }

    #[test]
    fn validate_name_matches_filename_accepts_match() {
        let content = "---\nname: perf\n---\nbody";
        let check = Check::parse(content, &p("/r/.agents/checks/perf.md")).unwrap();
        check.validate_name_matches_filename().unwrap();
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let err = Check::parse("just a body", &p("/r/.agents/checks/x.md")).unwrap_err();
        assert!(err.to_string().contains("missing frontmatter"));
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let err = Check::parse("---\nname: x\nno close", &p("/r/.agents/checks/x.md")).unwrap_err();
        assert!(err.to_string().contains("missing closing"));
    }

    #[test]
    fn parses_crlf_frontmatter() {
        let content = "---\r\nname: perf\r\ndescription: ok\r\n---\r\nbody line\r\n";
        let check = Check::parse(content, &p("/r/.agents/checks/perf.md")).unwrap();
        assert_eq!(check.name, "perf");
        assert_eq!(check.description.as_deref(), Some("ok"));
        assert_eq!(check.body, "body line");
    }

    #[test]
    fn resolves_model_precedence() {
        let mut check = Check::parse(
            "---\nname: x\nmodel: per-check\n---\n",
            &p("/r/.agents/checks/x.md"),
        )
        .unwrap();
        assert_eq!(
            check.resolved_model(Some("default"), None),
            Some("per-check")
        );
        assert_eq!(
            check.resolved_model(Some("default"), Some("override")),
            Some("override")
        );
        check.model = None;
        assert_eq!(check.resolved_model(Some("default"), None), Some("default"));
        assert_eq!(check.resolved_model(None, None), None);
    }

    #[test]
    fn resolves_turn_limit() {
        let mut check = Check::parse(
            "---\nname: x\nturn-limit: 7\n---\n",
            &p("/r/.agents/checks/x.md"),
        )
        .unwrap();
        assert_eq!(check.resolved_turn_limit(None), 7);
        assert_eq!(check.resolved_turn_limit(Some(99)), 7);
        check.turn_limit = None;
        assert_eq!(check.resolved_turn_limit(Some(99)), 99);
        assert_eq!(check.resolved_turn_limit(None), DEFAULT_CHECK_TURN_LIMIT);
    }

    #[test]
    fn candidate_scopes_walks_parents() {
        let scopes = candidate_scope_dirs(&[
            "api/v2/foo.rs".into(),
            "api/v2/bar.rs".into(),
            "README.md".into(),
        ]);
        assert_eq!(scopes, vec!["api".to_string(), "api/v2".to_string()]);
    }

    #[test]
    fn discovers_root_and_scoped_checks() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join(".agents/checks/sql.md"),
            "---\nname: sql\ndescription: root sql\n---\nbody",
        );
        write(
            &root.join("api/.agents/checks/auth.md"),
            "---\nname: auth\n---\nauth body",
        );

        let result = discover_with_globals(root, &["api/users.rs".to_string()], &[]).unwrap();
        let names: Vec<_> = result.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["auth", "sql"]);
    }

    #[test]
    fn closer_scope_overrides_same_named_check() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join(".agents/checks/perf.md"),
            "---\nname: perf\ndescription: root\n---\nroot body",
        );
        write(
            &root.join("api/.agents/checks/perf.md"),
            "---\nname: perf\ndescription: scoped\n---\nscoped body",
        );

        let result = discover_with_globals(root, &["api/users.rs".to_string()], &[]).unwrap();
        assert_eq!(result.checks.len(), 1);
        let perf = &result.checks[0];
        assert_eq!(perf.scope_dir, "api");
        assert_eq!(perf.body, "scoped body");
    }

    #[test]
    fn synthesizes_virtual_checks_for_review_md_at_each_scope() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join(".agents/REVIEW.md"), "root rules");
        write(&root.join("api/.agents/REVIEW.md"), "api rules");
        write(&root.join("api/v2/.agents/REVIEW.md"), "v2 rules");

        let result = discover_with_globals(root, &["api/v2/x.rs".to_string()], &[]).unwrap();
        let names: Vec<_> = result.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["repo-rules", "repo-rules:api", "repo-rules:api/v2"]
        );

        let root_check = result
            .checks
            .iter()
            .find(|c| c.name == "repo-rules")
            .unwrap();
        assert!(root_check.body.contains("the entire repository"));
        assert!(root_check.body.contains("root rules"));

        let scoped = result
            .checks
            .iter()
            .find(|c| c.name == "repo-rules:api/v2")
            .unwrap();
        assert!(scoped.body.contains("files under `api/v2/`"));
        assert!(scoped.body.contains("v2 rules"));
    }

    #[test]
    fn repo_root_check_overrides_same_named_global_check() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let global = tempdir().unwrap();
        write(
            &global.path().join("perf.md"),
            "---\nname: perf\ndescription: global\n---\nglobal body",
        );
        write(
            &root.join(".agents/checks/perf.md"),
            "---\nname: perf\ndescription: repo\n---\nrepo body",
        );

        let result = discover_with_globals(root, &[], &[global.path().to_path_buf()]).unwrap();
        assert_eq!(result.checks.len(), 1);
        assert_eq!(result.checks[0].body, "repo body");
        assert_eq!(result.checks[0].description.as_deref(), Some("repo"));
    }

    #[cfg(unix)]
    #[test]
    fn allows_symlinked_global_check_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        let global = dir.path().join("global");
        write(
            &global.join("perf.md"),
            "---\nname: perf\ndescription: global\n---\nglobal body",
        );
        let global_link = dir.path().join("global-link");
        symlink(&global, &global_link).unwrap();

        let result = discover_with_globals(&root, &[], std::slice::from_ref(&global_link)).unwrap();

        assert_eq!(result.checks.len(), 1);
        assert_eq!(result.checks[0].body, "global body");
        assert_eq!(result.checks[0].path, global_link.join("perf.md"));
    }

    #[test]
    fn user_check_named_repo_rules_is_not_overwritten_by_root_review_md() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join(".agents/checks/repo-rules.md"),
            "---\nname: repo-rules\ndescription: user\n---\nuser body",
        );
        write(&root.join(".agents/REVIEW.md"), "review rules");

        let result = discover_with_globals(root, &[], &[]).unwrap();
        let repo_rules = result
            .checks
            .iter()
            .find(|c| c.name == "repo-rules")
            .expect("repo-rules check should exist");
        assert_eq!(repo_rules.body, "user body");
    }

    #[test]
    fn skips_non_markdown_in_checks_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join(".agents/checks/README.txt"), "ignored");
        write(
            &root.join(".agents/checks/perf.md"),
            "---\nname: perf\n---\nbody",
        );
        let result = discover_with_globals(root, &[], &[]).unwrap();
        assert_eq!(result.checks.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_check_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        let external_checks = dir.path().join("external-checks");
        write(
            &external_checks.join("secret.md"),
            "---\nname: secret\n---\nexternal body",
        );
        fs::create_dir_all(root.join(".agents")).unwrap();
        symlink(&external_checks, root.join(".agents/checks")).unwrap();

        let result = discover_with_globals(&root, &[], &[]).unwrap();

        assert!(result.checks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn skips_checks_behind_symlinked_agent_directory() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        let external_agents = dir.path().join("external-agents");
        write(
            &external_agents.join("checks/secret.md"),
            "---\nname: secret\n---\nexternal body",
        );
        fs::create_dir_all(&root).unwrap();
        symlink(&external_agents, root.join(".agents")).unwrap();

        let result = discover_with_globals(&root, &[], &[]).unwrap();

        assert!(result.checks.is_empty());
    }

    #[test]
    fn strict_project_check_read_failures_are_not_skipped() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        let check = root.join(".agents/checks/oversized.md");
        let oversized = "x".repeat(crate::scheduler::MAX_SCHEDULE_RECIPE_BYTES as usize + 1);
        write(&check, &oversized);

        let error = discover_with_globals(&root, &[], &[]).unwrap_err();

        assert!(error.to_string().contains("read check file"));
    }

    #[cfg(unix)]
    #[test]
    fn skips_review_md_symlinks_outside_repo() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join(".agents")).unwrap();
        fs::create_dir_all(root.join("api/.agents")).unwrap();
        let root_secret = dir.path().join("root-secret.txt");
        let scoped_secret = dir.path().join("scoped-secret.txt");
        fs::write(&root_secret, "root secret").unwrap();
        fs::write(&scoped_secret, "scoped secret").unwrap();
        symlink(&root_secret, root.join(".agents/REVIEW.md")).unwrap();
        symlink(&scoped_secret, root.join("api/.agents/REVIEW.md")).unwrap();

        let result = discover_with_globals(&root, &["api/users.rs".to_string()], &[]).unwrap();

        assert!(result.checks.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn allows_review_md_symlinks_within_repo() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".agents")).unwrap();
        fs::write(root.join("review-rules.md"), "review rules").unwrap();
        symlink(root.join("review-rules.md"), root.join(".agents/REVIEW.md")).unwrap();

        let result = discover_with_globals(root, &[], &[]).unwrap();

        assert_eq!(result.checks.len(), 1);
        assert!(result.checks[0].body.ends_with("review rules"));
    }
}
