//! Filesystem-backed CRUD for [`SourceEntry`] values exchanged over ACP custom
//! methods. Skills live in `~/.agents/skills/` (or per-project under
//! `<project>/.agents/skills/`). Projects live in `<dataDir>/projects/<slug>.md`.

use crate::config::paths::Paths;
use crate::skills::{
    build_skill_md, discover_skills, infer_skill_name, is_global_skill_dir,
    parse_skill_frontmatter, resolve_discoverable_skill_dir, resolve_skill_dir, skill_base_dir,
    validate_skill_name,
};
use crate::source_roots::SourceRoot;
use agent_client_protocol::Error;
use fs_err as fs;
use goose_sdk_types::custom_requests::{SourceEntry, SourceType};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;

pub fn parse_frontmatter<T: for<'de> Deserialize<'de>>(
    content: &str,
) -> Result<Option<(T, String)>, serde_yaml::Error> {
    let parts: Vec<&str> = content.split("---").collect();
    if parts.len() < 3 {
        return Ok(None);
    }

    let yaml_content = parts[1].trim();
    let metadata: T = serde_yaml::from_str(yaml_content)?;

    let body = parts[2..].join("---").trim().to_string();
    Ok(Some((metadata, body)))
}

fn require_mutable_type(source_type: SourceType) -> Result<(), Error> {
    match source_type {
        SourceType::Skill | SourceType::Project | SourceType::Agent => Ok(()),
        other => Err(Error::invalid_params().data(format!(
            "Source type '{other}' is not supported for mutation."
        ))),
    }
}

fn require_listable_type(source_type: Option<SourceType>) -> Result<SourceType, Error> {
    match source_type.unwrap_or(SourceType::Skill) {
        SourceType::Skill => Ok(SourceType::Skill),
        SourceType::BuiltinSkill => Ok(SourceType::BuiltinSkill),
        SourceType::Project => Ok(SourceType::Project),
        SourceType::Agent => Ok(SourceType::Agent),
        other => Err(Error::invalid_params().data(format!(
            "Source type '{}' is not supported for listing.",
            other
        ))),
    }
}

// --- Project helpers ---

#[derive(Deserialize)]
struct MarkdownSourceFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, flatten)]
    properties: HashMap<String, serde_json::Value>,
}

fn projects_dir() -> PathBuf {
    Paths::data_dir().join("projects")
}

fn project_file_path(slug: &str) -> PathBuf {
    projects_dir().join(format!("{slug}.md"))
}

fn read_source_path(path: &Path) -> std::io::Result<String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path has no file name",
        )
    })?;
    crate::skills::read_source_file(&parent, Path::new(file_name))
}

fn build_source_markdown(
    name: &str,
    description: &str,
    content: &str,
    properties: &HashMap<String, serde_json::Value>,
) -> Result<String, Error> {
    let mut frontmatter = serde_yaml::Mapping::new();
    frontmatter.insert(
        serde_yaml::Value::String("name".into()),
        serde_yaml::Value::String(name.into()),
    );
    frontmatter.insert(
        serde_yaml::Value::String("description".into()),
        serde_yaml::Value::String(description.into()),
    );
    for (key, value) in properties {
        if key == "name" || key == "description" {
            continue;
        }
        let value = serde_yaml::to_value(value).map_err(|e| {
            Error::internal_error().data(format!("Failed to serialize source property: {e}"))
        })?;
        frontmatter.insert(serde_yaml::Value::String(key.clone()), value);
    }
    let yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|e| Error::internal_error().data(format!("Failed to serialize source: {e}")))?;
    let mut md = format!("---\n{yaml}---\n");
    if !content.is_empty() {
        md.push('\n');
        md.push_str(content);
        md.push('\n');
    }
    Ok(md)
}

/// Returns (display_name, description, body, properties).
fn parse_project_frontmatter(
    raw: &str,
) -> (String, String, String, HashMap<String, serde_json::Value>) {
    if !raw.trim_start().starts_with("---") {
        return (
            String::new(),
            String::new(),
            raw.to_string(),
            HashMap::new(),
        );
    }
    match parse_frontmatter::<MarkdownSourceFrontmatter>(raw) {
        Ok(Some((meta, body))) => (meta.name, meta.description, body, meta.properties),
        _ => (
            String::new(),
            String::new(),
            raw.to_string(),
            HashMap::new(),
        ),
    }
}

/// Validate a project slug. Same shape as a skill name (kebab-case, ASCII).
fn validate_project_slug(slug: &str) -> Result<(), Error> {
    validate_skill_name(slug)
}

/// Read the `metadata:` field out of an existing SKILL.md, returning an
/// empty map if the file is missing, malformed, or carries no metadata.
fn read_existing_skill_properties(skill_dir: &Path) -> HashMap<String, serde_json::Value> {
    let raw = match read_source_path(&skill_dir.join("SKILL.md")) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    match parse_frontmatter::<crate::skills::SkillFrontmatter>(&raw) {
        Ok(Some((meta, _))) => meta.metadata,
        _ => HashMap::new(),
    }
}

/// Read the properties bag out of an existing project file.
fn read_existing_project_properties(file: &Path) -> HashMap<String, serde_json::Value> {
    let raw = match read_source_path(file) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let (_, _, _, properties) = parse_project_frontmatter(&raw);
    properties
}

/// Read the properties bag out of an existing agent file.
fn read_existing_agent_properties(file: &Path) -> HashMap<String, serde_json::Value> {
    let raw = match read_source_path(file) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    match parse_agent_frontmatter(&raw) {
        Ok((frontmatter, _)) => frontmatter.properties,
        Err(_) => HashMap::new(),
    }
}

fn project_entry_from_file(file: &Path) -> Option<SourceEntry> {
    let slug = file.file_stem().and_then(|s| s.to_str())?.to_string();
    if slug.is_empty() {
        return None;
    }
    let raw = read_source_path(file).ok()?;
    let (title, description, content, mut properties) = parse_project_frontmatter(&raw);
    let display_name = if title.is_empty() {
        slug.clone()
    } else {
        title
    };
    if display_name != slug {
        // Preserve the user-facing display name so the frontend doesn't have
        // to special-case slug vs title.
        properties.insert(
            "title".into(),
            serde_json::Value::String(display_name.clone()),
        );
    }
    Some(SourceEntry {
        source_type: SourceType::Project,
        name: slug,
        description,
        content,
        path: file.to_string_lossy().into_owned(),
        global: true,
        writable: true,
        supporting_files: Vec::new(),
        properties,
    })
}

/// Read all projects from `<dataDir>/projects/`.
fn read_project_dir() -> Result<Vec<SourceEntry>, Error> {
    let dir = projects_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&dir)
        .map_err(|e| Error::internal_error().data(format!("Failed to read projects dir: {e}")))?;

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Some(entry) = project_entry_from_file(&path) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Read a single project source by slug.
pub fn read_project(slug: &str) -> Result<SourceEntry, Error> {
    validate_project_slug(slug)?;
    let file = project_file_path(slug);
    if !file.exists() {
        return Err(Error::invalid_params().data(format!("Project \"{}\" not found", slug)));
    }
    project_entry_from_file(&file)
        .ok_or_else(|| Error::internal_error().data("Failed to read project file"))
}

/// Get the working directories configured for a project, if any.
/// Returns an empty Vec when the project doesn't exist or has none configured.
pub fn project_working_dirs(slug: &str) -> Vec<String> {
    let entry = match read_project(slug) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entry
        .properties
        .get("workingDirs")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default()
}

/// Validate that the given path is a project file we manage and the file
/// exists. Returns the canonical path on success.
fn resolve_project_path(path: &str) -> Result<PathBuf, Error> {
    let canonical_path = Path::new(path).canonicalize().map_err(|_| {
        Error::invalid_params().data(format!("Project source \"{}\" not found", path))
    })?;
    let canonical_root = projects_dir()
        .canonicalize()
        .unwrap_or_else(|_| projects_dir());
    if !canonical_path.starts_with(&canonical_root) {
        return Err(Error::invalid_params().data(format!(
            "Path \"{}\" is not a project source",
            canonical_path.display()
        )));
    }
    if canonical_path.extension().and_then(|e| e.to_str()) != Some("md") {
        return Err(
            Error::invalid_params().data(format!("Path \"{}\" is not a markdown file", path))
        );
    }
    if !canonical_path.is_file() {
        return Err(Error::invalid_params().data(format!("Project source \"{}\" not found", path)));
    }
    Ok(canonical_path)
}

// --- SourceEntry construction ---

fn skill_source_entry(
    name: &str,
    description: &str,
    content: &str,
    dir: &Path,
    global: bool,
    properties: HashMap<String, serde_json::Value>,
) -> SourceEntry {
    SourceEntry {
        source_type: SourceType::Skill,
        name: name.to_string(),
        description: description.to_string(),
        content: content.to_string(),
        path: dir.to_string_lossy().to_string(),
        global,
        writable: true,
        supporting_files: Vec::new(),
        properties,
    }
}

fn builtin_skill_entry(mut source: SourceEntry) -> SourceEntry {
    source.source_type = SourceType::BuiltinSkill;
    source.path = format!("builtin://skills/{}", source.name);
    source.global = true;
    source.supporting_files.clear();
    source
}

fn agent_base_dir(global: bool, project_dir: Option<&str>) -> Result<PathBuf, Error> {
    if global {
        Ok(Paths::agents_dir())
    } else {
        let project_dir = project_dir.ok_or_else(|| {
            Error::invalid_params().data("projectDir is required when global is false")
        })?;
        if project_dir.trim().is_empty() {
            return Err(
                Error::invalid_params().data("projectDir must not be empty when global is false")
            );
        }
        Ok(Path::new(project_dir).join(".agents").join("agents"))
    }
}

fn validate_agent_name(name: &str) -> Result<(), Error> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid_params().data("Agent name must not be empty"));
    }
    if trimmed.len() > 80 {
        return Err(Error::invalid_params().data(format!(
            "Invalid agent name \"{}\". Names must be at most 80 characters.",
            name
        )));
    }
    if trimmed.chars().any(|ch| matches!(ch, '/' | '\\')) {
        return Err(Error::invalid_params().data(format!(
            "Invalid agent name \"{}\". Names must not contain path separators.",
            name
        )));
    }
    Ok(())
}

fn slugify_agent_name(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let mut collapsed = String::with_capacity(slug.len());
    let mut previous_hyphen = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !previous_hyphen {
                collapsed.push('-');
            }
            previous_hyphen = true;
        } else {
            collapsed.push(ch);
            previous_hyphen = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed
            .chars()
            .take(64)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    }
}

fn parse_agent_frontmatter(raw: &str) -> Result<(MarkdownSourceFrontmatter, String), Error> {
    parse_frontmatter::<MarkdownSourceFrontmatter>(raw)
        .map_err(|e| Error::invalid_params().data(format!("Invalid agent frontmatter: {e}")))?
        .ok_or_else(|| Error::invalid_params().data("Agent file is missing frontmatter"))
}

fn agent_source_entry(path: &Path, global: bool, writable: bool) -> Result<SourceEntry, Error> {
    let raw = read_source_path(path)
        .map_err(|e| Error::internal_error().data(format!("Failed to read agent file: {e}")))?;
    agent_source_entry_from_raw(&raw, path, global, writable)
}

fn agent_source_entry_from_raw(
    raw: &str,
    path: &Path,
    global: bool,
    writable: bool,
) -> Result<SourceEntry, Error> {
    let (frontmatter, content) = parse_agent_frontmatter(raw)?;
    Ok(SourceEntry {
        source_type: SourceType::Agent,
        name: frontmatter.name,
        description: frontmatter.description,
        content,
        path: path.to_string_lossy().to_string(),
        global,
        writable,
        supporting_files: Vec::new(),
        properties: frontmatter.properties,
    })
}

fn listed_agent_source_entry(
    root: &Path,
    relative_path: &Path,
    listed_path: &Path,
    global: bool,
    writable: bool,
) -> Result<SourceEntry, Error> {
    let raw = crate::skills::read_source_file(root, relative_path)
        .map_err(|e| Error::internal_error().data(format!("Failed to read agent file: {e}")))?;
    agent_source_entry_from_raw(&raw, listed_path, global, writable)
}

fn canonicalize_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_under_root(path: &Path, root: &Path) -> bool {
    canonicalize_or_original(path).starts_with(canonicalize_or_original(root))
}

fn is_read_only_agent_file(path: &Path, additional_roots: &[SourceRoot]) -> bool {
    additional_roots
        .iter()
        .filter(|root| !root.writable)
        .any(|root| is_under_root(path, &root.path))
}

fn reject_read_only_agent_file(path: &Path, additional_roots: &[SourceRoot]) -> Result<(), Error> {
    if is_read_only_agent_file(path, additional_roots) {
        return Err(Error::invalid_params().data("Source is read-only"));
    }
    Ok(())
}

fn is_global_agent_file(path: &Path) -> bool {
    let canonical_path = canonicalize_or_original(path);
    let mut global_roots = Vec::new();
    global_roots.push(Paths::agents_dir());
    if let Some(home) = dirs::home_dir() {
        global_roots.push(home.join(".agents").join("agents"));
        global_roots.push(home.join(".goose").join("agents"));
        global_roots.push(home.join(".claude").join("agents"));
    }
    global_roots.push(Paths::config_dir().join("agents"));

    global_roots
        .into_iter()
        .any(|root| canonical_path.starts_with(canonicalize_or_original(&root)))
}

fn resolve_agent_file_with_roots(
    path: &str,
    additional_roots: &[SourceRoot],
) -> Result<PathBuf, Error> {
    if path.is_empty() {
        return Err(Error::invalid_params().data("Source path must not be empty"));
    }

    let canonical_file = Path::new(path)
        .canonicalize()
        .map_err(|_| Error::invalid_params().data(format!("Source \"{}\" not found", path)))?;

    let parent_name = canonical_file
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let grandparent_name = canonical_file
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let in_agent_dir = parent_name == Some("agents")
        && matches!(
            grandparent_name,
            Some(".goose") | Some(".claude") | Some(".agents")
        );
    let in_additional_root = additional_roots
        .iter()
        .any(|root| is_under_root(&canonical_file, &root.path));

    if !canonical_file.is_file()
        || canonical_file.extension().and_then(|ext| ext.to_str()) != Some("md")
        || (!in_agent_dir && !is_global_agent_file(&canonical_file) && !in_additional_root)
    {
        return Err(Error::invalid_params().data(format!("Source \"{}\" not found", path)));
    }

    Ok(canonical_file)
}

fn list_agent_dirs(
    working_dir: Option<&Path>,
    additional_roots: &[SourceRoot],
) -> Vec<(SourceRoot, bool)> {
    let mut dirs = Vec::new();
    {
        let mut push = |path, global| {
            dirs.push((
                SourceRoot {
                    path,
                    writable: true,
                },
                global,
            ));
        };
        if let Some(working_dir) = working_dir {
            push(working_dir.join(".agents").join("agents"), false);
            push(working_dir.join(".goose").join("agents"), false);
            push(working_dir.join(".claude").join("agents"), false);
        }

        push(Paths::agents_dir(), true);
        if let Some(home) = dirs::home_dir() {
            push(home.join(".agents").join("agents"), true);
            push(home.join(".goose").join("agents"), true);
            push(home.join(".claude").join("agents"), true);
        }
        push(Paths::config_dir().join("agents"), true);
    }
    dirs.extend(additional_roots.iter().cloned().map(|root| (root, true)));
    dirs
}

fn list_agent_sources(
    project_dir: Option<&str>,
    additional_roots: &[SourceRoot],
) -> Vec<SourceEntry> {
    let working_dir = project_dir
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let canonical_working_dir = working_dir
        .as_deref()
        .and_then(|working_dir| working_dir.canonicalize().ok());
    let mut seen = std::collections::HashSet::new();
    let mut sources = Vec::new();

    for (root, global) in list_agent_dirs(working_dir.as_deref(), additional_roots) {
        let canonical_root = match root.path.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !global
            && canonical_working_dir
                .as_deref()
                .is_none_or(|working_dir| !canonical_root.starts_with(working_dir))
        {
            warn!(
                "Skipping project agent root outside the project: {}",
                root.path.display()
            );
            continue;
        }
        let entries = match fs::read_dir(&canonical_root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let listed_path = root.path.join(entry.file_name());
            if listed_path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let canonical_path = match entry.path().canonicalize() {
                Ok(path) if path.is_file() => path,
                _ => continue,
            };
            let Ok(relative_path) = canonical_path.strip_prefix(&canonical_root) else {
                continue;
            };
            let writable = root.writable
                && resolve_agent_file_with_roots(&listed_path.to_string_lossy(), additional_roots)
                    .is_ok();
            match listed_agent_source_entry(
                &canonical_root,
                relative_path,
                &listed_path,
                global,
                writable,
            ) {
                Ok(source) => {
                    let key = source.name.to_lowercase();
                    if seen.insert(key) {
                        sources.push(source);
                    }
                }
                Err(err) => warn!("Skipping agent source {}: {:?}", listed_path.display(), err),
            }
        }
    }

    sources
}

fn create_agent_source(
    name: &str,
    description: &str,
    content: &str,
    properties: HashMap<String, serde_json::Value>,
    global: bool,
    project_dir: Option<&str>,
) -> Result<SourceEntry, Error> {
    validate_agent_name(name)?;
    let base = agent_base_dir(global, project_dir)?;
    let slug = slugify_agent_name(name);
    let mut file_path = base.join(format!("{slug}.md"));
    if file_path.exists() {
        let mut counter = 2u32;
        loop {
            file_path = base.join(format!("{slug}-{counter}.md"));
            if !file_path.exists() {
                break;
            }
            counter += 1;
        }
    }

    fs::create_dir_all(&base).map_err(|e| {
        Error::internal_error().data(format!("Failed to create source directory: {e}"))
    })?;
    let md = build_source_markdown(name, description, content, &properties)?;
    fs::write(&file_path, md)
        .map_err(|e| Error::internal_error().data(format!("Failed to write agent file: {e}")))?;

    agent_source_entry(&file_path, global, true)
}

fn update_agent_source(
    path: &str,
    name: &str,
    description: &str,
    content: &str,
    properties: Option<HashMap<String, serde_json::Value>>,
    additional_roots: &[SourceRoot],
) -> Result<SourceEntry, Error> {
    validate_agent_name(name)?;
    let file_path = resolve_agent_file_with_roots(path, additional_roots)?;
    reject_read_only_agent_file(&file_path, additional_roots)?;
    let global = is_global_agent_file(&file_path);
    let resolved_properties = match properties {
        Some(p) => p,
        None => read_existing_agent_properties(&file_path),
    };
    let md = build_source_markdown(name, description, content, &resolved_properties)?;
    fs::write(&file_path, md)
        .map_err(|e| Error::internal_error().data(format!("Failed to write agent file: {e}")))?;

    agent_source_entry(&file_path, global, true)
}

// --- Public CRUD ---

pub fn create_source(
    source_type: SourceType,
    name: &str,
    description: &str,
    content: &str,
    global: bool,
    project_dir: Option<&str>,
    properties: HashMap<String, serde_json::Value>,
) -> Result<SourceEntry, Error> {
    require_mutable_type(source_type)?;
    if source_type == SourceType::Agent {
        return create_agent_source(name, description, content, properties, global, project_dir);
    }

    match source_type {
        SourceType::Skill => {
            validate_skill_name(name)?;
            let dir = skill_base_dir(global, project_dir)?.join(name);

            if dir.exists() {
                return Err(Error::invalid_params()
                    .data(format!("A source named \"{}\" already exists", name)));
            }

            fs::create_dir_all(&dir).map_err(|e| {
                Error::internal_error().data(format!("Failed to create source directory: {e}"))
            })?;
            let file_path = dir.join("SKILL.md");
            let md = build_skill_md(name, description, content, &properties);
            fs::write(&file_path, md).map_err(|e| {
                Error::internal_error().data(format!("Failed to write SKILL.md: {e}"))
            })?;

            Ok(skill_source_entry(
                name,
                description,
                content,
                &dir,
                global,
                properties,
            ))
        }
        SourceType::Project => {
            validate_project_slug(name)?;
            let base = projects_dir();
            fs::create_dir_all(&base).map_err(|e| {
                Error::internal_error().data(format!("Failed to create projects dir: {e}"))
            })?;
            let file = project_file_path(name);
            if file.exists() {
                return Err(Error::invalid_params()
                    .data(format!("A source named \"{}\" already exists", name)));
            }
            // The display name comes from `properties.title`; if absent, the
            // file's frontmatter `name:` is the slug itself.
            let display_name = properties
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(name);
            let md = build_source_markdown(display_name, description, content, &properties)?;
            fs::write(&file, md).map_err(|e| {
                Error::internal_error().data(format!("Failed to write project file: {e}"))
            })?;
            project_entry_from_file(&file)
                .ok_or_else(|| Error::internal_error().data("Failed to read newly created project"))
        }
        _ => unreachable!("guarded by require_mutable_type"),
    }
}

pub struct UpdateSourceOptions<'a> {
    pub properties: Option<HashMap<String, serde_json::Value>>,
    pub additional_roots: &'a [SourceRoot],
}

pub fn update_source_with_roots(
    source_type: SourceType,
    path: &str,
    name: &str,
    description: &str,
    content: &str,
    options: UpdateSourceOptions<'_>,
) -> Result<SourceEntry, Error> {
    require_mutable_type(source_type)?;
    if source_type == SourceType::Agent {
        return update_agent_source(
            path,
            name,
            description,
            content,
            options.properties,
            options.additional_roots,
        );
    }

    match source_type {
        SourceType::Skill => {
            validate_skill_name(name)?;

            let dir = resolve_discoverable_skill_dir(path)?;
            let current_dir_name = dir
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    Error::internal_error().data("Failed to resolve source directory name")
                })?;

            let resolved_properties = match options.properties {
                Some(p) => p,
                None => read_existing_skill_properties(&dir),
            };

            let target_dir = if name == current_dir_name {
                dir.clone()
            } else {
                let base_dir = dir.parent().ok_or_else(|| {
                    Error::internal_error().data("Failed to resolve source base directory")
                })?;
                let target_dir = base_dir.join(name);

                if target_dir.exists() {
                    return Err(Error::invalid_params()
                        .data(format!("A source named \"{}\" already exists", name)));
                }

                fs::rename(&dir, &target_dir).map_err(|e| {
                    Error::internal_error().data(format!("Failed to rename source directory: {e}"))
                })?;

                target_dir
            };

            let file_path = target_dir.join("SKILL.md");
            let md = build_skill_md(name, description, content, &resolved_properties);
            fs::write(&file_path, md).map_err(|e| {
                Error::internal_error().data(format!("Failed to write SKILL.md: {e}"))
            })?;

            Ok(skill_source_entry(
                name,
                description,
                content,
                &target_dir,
                is_global_skill_dir(&target_dir),
                resolved_properties,
            ))
        }
        SourceType::Project => {
            validate_project_slug(name)?;
            let file = resolve_project_path(path)?;

            let current_slug = file
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::internal_error().data("Bad project filename"))?;
            if current_slug != name {
                return Err(Error::invalid_params().data(format!(
                    "Project slug cannot be changed (current: \"{}\", requested: \"{}\")",
                    current_slug, name
                )));
            }

            let resolved_properties = match options.properties {
                Some(p) => p,
                None => read_existing_project_properties(&file),
            };

            let display_name = resolved_properties
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(name);
            let md =
                build_source_markdown(display_name, description, content, &resolved_properties)?;
            fs::write(&file, md).map_err(|e| {
                Error::internal_error().data(format!("Failed to write project file: {e}"))
            })?;
            project_entry_from_file(&file)
                .ok_or_else(|| Error::internal_error().data("Failed to read updated project"))
        }
        _ => unreachable!("guarded by require_mutable_type"),
    }
}

pub fn delete_source(source_type: SourceType, path: &str) -> Result<(), Error> {
    delete_source_with_roots(source_type, path, &[])
}

pub fn delete_source_with_roots(
    source_type: SourceType,
    path: &str,
    additional_roots: &[SourceRoot],
) -> Result<(), Error> {
    require_mutable_type(source_type)?;

    match source_type {
        SourceType::Skill => {
            let dir = resolve_skill_dir(path)?;
            fs::remove_dir_all(&dir).map_err(|e| {
                Error::internal_error().data(format!("Failed to delete source: {e}"))
            })?;
        }
        SourceType::Project => {
            let file = resolve_project_path(path)?;
            fs::remove_file(&file).map_err(|e| {
                Error::internal_error().data(format!("Failed to delete project: {e}"))
            })?;
        }
        SourceType::Agent => {
            let file_path = resolve_agent_file_with_roots(path, additional_roots)?;
            reject_read_only_agent_file(&file_path, additional_roots)?;
            fs::remove_file(&file_path).map_err(|e| {
                Error::internal_error().data(format!("Failed to delete source: {e}"))
            })?;
        }
        _ => unreachable!("guarded by require_mutable_type"),
    }
    Ok(())
}

pub fn list_sources(
    source_type: Option<SourceType>,
    project_dir: Option<&str>,
    include_project_sources: bool,
) -> Result<Vec<SourceEntry>, Error> {
    list_sources_with_roots(source_type, project_dir, include_project_sources, &[])
}

pub fn list_sources_with_roots(
    source_type: Option<SourceType>,
    project_dir: Option<&str>,
    include_project_sources: bool,
    additional_roots: &[SourceRoot],
) -> Result<Vec<SourceEntry>, Error> {
    if let Some(t) = source_type {
        require_listable_type(Some(t))?;
    }
    let kinds: Vec<SourceType> = match source_type {
        Some(t) => vec![t],
        None => vec![SourceType::Skill, SourceType::Project],
    };

    let mut sources = Vec::new();
    for kind in kinds {
        match kind {
            SourceType::Skill => {
                let working_dir = project_dir
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from);
                sources.extend(
                    discover_skills(working_dir.as_deref())
                        .into_iter()
                        .filter(|s| s.source_type == SourceType::Skill),
                );

                if include_project_sources {
                    let projects = read_project_dir()?;
                    let already_scanned = working_dir.as_deref();
                    for proj in &projects {
                        let dirs = proj
                            .properties
                            .get("workingDirs")
                            .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                            .unwrap_or_default();
                        let project_name = proj
                            .properties
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&proj.name);
                        for wd in &dirs {
                            let wd_path = PathBuf::from(wd);
                            if Some(wd_path.as_path()) == already_scanned {
                                continue;
                            }
                            for skill in discover_skills(Some(&wd_path)) {
                                if skill.source_type != SourceType::Skill || skill.global {
                                    continue;
                                }
                                let mut tagged = skill;
                                tagged.properties.insert(
                                    "projectName".into(),
                                    serde_json::Value::String(project_name.to_string()),
                                );
                                tagged.properties.insert(
                                    "projectDir".into(),
                                    serde_json::Value::String(wd.clone()),
                                );
                                sources.push(tagged);
                            }
                        }
                    }
                }
            }
            SourceType::BuiltinSkill => {
                let working_dir = project_dir
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from);
                sources.extend(
                    discover_skills(working_dir.as_deref())
                        .into_iter()
                        .filter(|s| s.source_type == SourceType::BuiltinSkill)
                        .map(builtin_skill_entry),
                );
            }
            SourceType::Project => {
                sources.extend(read_project_dir()?);
            }
            SourceType::Agent => {
                sources.extend(list_agent_sources(project_dir, additional_roots));

                // Surface `.agents/checks/*.md` review checks under the same
                // `Agent` source type. They live at a different path on disk
                // (Amp-compatible `.agents/checks/`) but are conceptually
                // agents: a check is a sub-agent definition specialized for
                // code review. `properties["kind"] = "check"` lets clients
                // differentiate.
                let working_dir = project_dir
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from);
                let discovered = match working_dir.as_deref() {
                    Some(root) => crate::checks::discover(root, &[])
                        .map_err(|e| Error::internal_error().data(e.to_string()))?,
                    None => crate::checks::DiscoveredReview::default(),
                };
                for check in discovered.checks {
                    let global = check.path.starts_with(
                        crate::checks::global_checks_dirs()
                            .first()
                            .map(PathBuf::as_path)
                            .unwrap_or_else(|| Path::new("")),
                    );
                    sources.push(check.to_source_entry(global));
                }
            }
            SourceType::Recipe | SourceType::Subrecipe => {
                return Err(Error::invalid_params()
                    .data(format!("Source type '{}' listing is not supported.", kind)));
            }
        }
    }

    sources.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sources)
}

pub fn export_source(source_type: SourceType, path: &str) -> Result<(String, String), Error> {
    export_source_with_roots(source_type, path, &[])
}

pub fn export_source_with_roots(
    source_type: SourceType,
    path: &str,
    additional_roots: &[SourceRoot],
) -> Result<(String, String), Error> {
    match source_type {
        SourceType::Skill => {
            let dir = resolve_discoverable_skill_dir(path)?;

            let md = dir.join("SKILL.md");
            let raw = read_source_path(&md).map_err(|e| {
                Error::internal_error().data(format!("Failed to read SKILL.md: {e}"))
            })?;
            let (description, content) = parse_skill_frontmatter(&raw);

            let name = infer_skill_name(&dir);

            let export = serde_json::json!({
                "version": 1,
                "type": "skill",
                "name": name,
                "description": description,
                "content": content,
            });
            let json = serde_json::to_string_pretty(&export).map_err(|e| {
                Error::internal_error().data(format!("Failed to serialize source: {e}"))
            })?;
            let filename = format!("{}.skill.json", name);
            Ok((json, filename))
        }
        SourceType::Agent => {
            let file_path = resolve_agent_file_with_roots(path, additional_roots)?;
            let writable = !is_read_only_agent_file(&file_path, additional_roots);
            let source = agent_source_entry(
                &file_path,
                is_global_agent_file(&file_path) || !writable,
                writable,
            )?;
            let export = serde_json::json!({
                "version": 1,
                "type": "agent",
                "name": source.name,
                "description": source.description,
                "content": source.content,
            });
            let json = serde_json::to_string_pretty(&export).map_err(|e| {
                Error::internal_error().data(format!("Failed to serialize source: {e}"))
            })?;
            let filename = format!("{}.agent.json", slugify_agent_name(&source.name));
            Ok((json, filename))
        }
        SourceType::Project => {
            let file = resolve_project_path(path)?;
            let raw = read_source_path(&file).map_err(|e| {
                Error::internal_error().data(format!("Failed to read project file: {e}"))
            })?;
            let (title, description, content, properties) = parse_project_frontmatter(&raw);
            let slug = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let display_name = if title.is_empty() {
                slug.clone()
            } else {
                title
            };

            let mut export = serde_json::json!({
                "version": 1,
                "type": "project",
                "name": slug,
                "title": display_name,
                "description": description,
                "content": content,
            });
            if !properties.is_empty() {
                export["properties"] = serde_json::to_value(&properties).unwrap_or_default();
            }
            let json = serde_json::to_string_pretty(&export).map_err(|e| {
                Error::internal_error().data(format!("Failed to serialize project: {e}"))
            })?;
            let filename = format!("{}.project.json", slug);
            Ok((json, filename))
        }
        _ => Err(Error::invalid_params().data(format!(
            "Source type '{}' export is not supported.",
            source_type
        ))),
    }
}

pub fn import_sources(
    data: &str,
    global: bool,
    project_dir: Option<&str>,
) -> Result<Vec<SourceEntry>, Error> {
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| Error::invalid_params().data(format!("Invalid JSON: {e}")))?;

    let version = value
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::invalid_params().data("Missing or invalid \"version\" field"))?;
    if version != 1 {
        return Err(
            Error::invalid_params().data(format!("Unsupported source export version: {}", version))
        );
    }

    let type_str = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("skill");
    let source_type = match type_str {
        "skill" => SourceType::Skill,
        "project" => SourceType::Project,
        "agent" => SourceType::Agent,
        other => {
            return Err(Error::invalid_params()
                .data(format!("Source type '{}' import is not supported.", other)));
        }
    };

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::invalid_params().data("Missing or invalid \"name\" field"))?
        .to_string();
    if name.is_empty() {
        return Err(Error::invalid_params().data("Source name must not be empty"));
    }

    // Skills require a description; projects can omit it.
    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if source_type == SourceType::Skill && description.is_empty() {
        return Err(Error::invalid_params().data("Source description must not be empty"));
    }

    let content = value
        .get("content")
        .or_else(|| value.get("instructions"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut properties: HashMap<String, serde_json::Value> = value
        .get("properties")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    // The export's top-level "title" wins over a properties.title if both
    // exist.
    if source_type == SourceType::Project {
        if let Some(title) = value.get("title").and_then(|v| v.as_str()) {
            if !title.is_empty() {
                properties.insert("title".into(), serde_json::Value::String(title.into()));
            }
        }
    }

    if source_type == SourceType::Agent {
        if let Some(legacy_metadata) = value.get("metadata").and_then(|v| v.as_object()) {
            for (key, value) in legacy_metadata {
                properties
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        return create_agent_source(
            &name,
            &description,
            &content,
            properties,
            global,
            project_dir,
        )
        .map(|source| vec![source]);
    }

    match source_type {
        SourceType::Skill => {
            validate_skill_name(&name)?;
            let base = skill_base_dir(global, project_dir)?;
            let mut final_name = name.clone();
            if base.join(&final_name).exists() {
                final_name = format!("{}-imported", name);
                let mut counter = 2u32;
                while base.join(&final_name).exists() {
                    final_name = format!("{}-imported-{}", name, counter);
                    counter += 1;
                }
            }
            create_source(
                SourceType::Skill,
                &final_name,
                &description,
                &content,
                global,
                project_dir,
                properties,
            )
            .map(|entry| vec![entry])
        }
        SourceType::Project => {
            validate_project_slug(&name)?;
            let mut final_name = name.clone();
            if project_file_path(&final_name).exists() {
                final_name = format!("{}-imported", name);
                let mut counter = 2u32;
                while project_file_path(&final_name).exists() {
                    final_name = format!("{}-imported-{}", name, counter);
                    counter += 1;
                }
            }
            create_source(
                SourceType::Project,
                &final_name,
                &description,
                &content,
                true,
                None,
                properties,
            )
            .map(|entry| vec![entry])
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_agent(path: &Path, name: &str, description: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{name} instructions"),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn project_agent_sources_reject_external_symlinked_roots() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        for source_dir in [".agents", ".goose", ".claude"] {
            let external_root = tmp.path().join(format!("external-{source_dir}"));
            write_agent(
                &external_root.join(format!("{source_dir}.md")),
                source_dir,
                "external",
            );
            let project_source_dir = project.join(source_dir);
            std::fs::create_dir_all(&project_source_dir).unwrap();
            symlink(external_root, project_source_dir.join("agents")).unwrap();
        }

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();

        assert!(!listed
            .iter()
            .any(|source| { matches!(source.name.as_str(), ".agents" | ".goose" | ".claude") }));
    }

    #[cfg(unix)]
    #[test]
    fn project_agent_sources_reject_external_symlinked_files() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        for source_dir in [".agents", ".goose", ".claude"] {
            let external_agent = tmp.path().join(format!("external-{source_dir}.md"));
            write_agent(&external_agent, source_dir, "external");
            let agent_root = project.join(source_dir).join("agents");
            std::fs::create_dir_all(&agent_root).unwrap();
            symlink(external_agent, agent_root.join(format!("{source_dir}.md"))).unwrap();
        }

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();

        assert!(!listed
            .iter()
            .any(|source| { matches!(source.name.as_str(), ".agents" | ".goose" | ".claude") }));
    }

    #[cfg(unix)]
    #[test]
    fn project_agent_sources_allow_symlinks_contained_by_the_same_root() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let agent_root = project.join(".agents").join("agents");
        let contained_agent = agent_root.join("nested").join("contained.md");
        write_agent(&contained_agent, "Contained", "contained agent");
        let listed_path = agent_root.join("contained.md");
        symlink(&contained_agent, &listed_path).unwrap();

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();

        let contained = listed
            .iter()
            .find(|source| source.name == "Contained")
            .unwrap();
        assert!(!contained.global);
        assert!(!contained.writable);
        assert_eq!(contained.path, listed_path.to_string_lossy());
        assert_eq!(contained.content, "Contained instructions");

        let update_err = update_source_with_roots(
            SourceType::Agent,
            &contained.path,
            "Contained",
            "updated",
            "updated instructions",
            UpdateSourceOptions {
                properties: None,
                additional_roots: &[],
            },
        )
        .unwrap_err();
        assert!(format!("{update_err:?}").contains("not found"));

        let export_err = export_source(SourceType::Agent, &contained.path).unwrap_err();
        assert!(format!("{export_err:?}").contains("not found"));

        let delete_err = delete_source(SourceType::Agent, &contained.path).unwrap_err();
        assert!(format!("{delete_err:?}").contains("not found"));
        assert!(listed_path.exists());
        assert!(contained_agent.exists());
        assert!(std::fs::read_to_string(contained_agent)
            .unwrap()
            .contains("contained agent"));
    }

    #[test]
    fn project_agent_sources_keep_direct_files_writable() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let agent_path = project.join(".agents").join("agents").join("direct.md");
        write_agent(&agent_path, "Direct", "direct agent");

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();
        let direct = listed
            .iter()
            .find(|source| source.name == "Direct")
            .unwrap();
        assert!(direct.writable);

        let (exported, filename) = export_source(SourceType::Agent, &direct.path).unwrap();
        assert_eq!(filename, "direct.agent.json");
        assert!(exported.contains("\"name\": \"Direct\""));

        let updated = update_source_with_roots(
            SourceType::Agent,
            &direct.path,
            "Direct",
            "updated direct agent",
            "updated instructions",
            UpdateSourceOptions {
                properties: None,
                additional_roots: &[],
            },
        )
        .unwrap();
        assert_eq!(updated.description, "updated direct agent");
        assert_eq!(updated.content, "updated instructions");

        delete_source(SourceType::Agent, &direct.path).unwrap();
        assert!(!agent_path.exists());
    }

    #[test]
    fn project_agent_sources_preserve_directory_precedence() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        for (source_dir, unique_name) in [
            (".agents", "Agents unique"),
            (".goose", "Goose unique"),
            (".claude", "Claude unique"),
        ] {
            let agent_root = project.join(source_dir).join("agents");
            write_agent(&agent_root.join("unique.md"), unique_name, "unique agent");
            write_agent(
                &agent_root.join("shared.md"),
                "Shared",
                &format!("from {source_dir}"),
            );
        }

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();

        for name in ["Agents unique", "Goose unique", "Claude unique"] {
            let source = listed.iter().find(|source| source.name == name).unwrap();
            assert!(!source.global);
        }
        let shared: Vec<_> = listed
            .iter()
            .filter(|source| source.name == "Shared")
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].description, "from .agents");
    }

    #[test]
    fn skill_name_validation() {
        assert!(validate_skill_name("my-skill").is_ok());
        assert!(validate_skill_name("abc123").is_ok());
        assert!(validate_skill_name("double--hyphen").is_ok());
        assert!(validate_skill_name("").is_err());
        assert!(validate_skill_name("-leading").is_err());
        assert!(validate_skill_name("trailing-").is_err());
        assert!(validate_skill_name("CAPS").is_err());
        assert!(validate_skill_name("../escape").is_err());
        assert!(validate_skill_name(&"a".repeat(64)).is_ok());
        assert!(validate_skill_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn lists_additional_read_only_agent_roots() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("builtin").join("agents");
        std::fs::create_dir_all(&root).unwrap();
        let agent_path = root.join("solo.md");
        std::fs::write(
            &agent_path,
            "---\nname: Solo\ndescription: Built in\n---\n\nYou are Solo.",
        )
        .unwrap();

        let sources = list_sources_with_roots(
            Some(SourceType::Agent),
            None,
            false,
            &[SourceRoot::read_only(root.clone())],
        )
        .unwrap();

        let solo = sources.iter().find(|source| source.name == "Solo").unwrap();
        assert!(!solo.writable);
        assert!(solo.global);
        assert_eq!(solo.path, agent_path.to_string_lossy());

        let err = update_source_with_roots(
            SourceType::Agent,
            &solo.path,
            "Solo",
            "Built in",
            "Updated",
            UpdateSourceOptions {
                properties: None,
                additional_roots: &[SourceRoot::read_only(root.canonicalize().unwrap())],
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("read-only"));

        let err = delete_source_with_roots(
            SourceType::Agent,
            &solo.path,
            &[SourceRoot::read_only(root.canonicalize().unwrap())],
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("read-only"));
    }

    #[test]
    fn additional_writable_agent_roots_keep_crud_access() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("custom").join("agents");
        let agent_path = root.join("solo.md");
        write_agent(&agent_path, "Solo", "custom agent");
        let roots = [SourceRoot {
            path: root,
            writable: true,
        }];

        let sources =
            list_sources_with_roots(Some(SourceType::Agent), None, false, &roots).unwrap();
        let solo = sources.iter().find(|source| source.name == "Solo").unwrap();
        assert!(solo.writable);
        assert!(solo.global);

        export_source_with_roots(SourceType::Agent, &solo.path, &roots).unwrap();
        update_source_with_roots(
            SourceType::Agent,
            &solo.path,
            "Solo",
            "updated custom agent",
            "updated instructions",
            UpdateSourceOptions {
                properties: None,
                additional_roots: &roots,
            },
        )
        .unwrap();
        delete_source_with_roots(SourceType::Agent, &solo.path, &roots).unwrap();
        assert!(!agent_path.exists());
    }

    #[test]
    fn create_list_update_delete_project_skill() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().to_str().unwrap();

        let created = create_source(
            SourceType::Skill,
            "my-skill",
            "does the thing",
            "step one\nstep two",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap();
        assert_eq!(created.name, "my-skill");
        assert!(!created.global);
        let dir = PathBuf::from(&created.path);
        assert!(dir.join("SKILL.md").exists());

        let listed = list_sources(Some(SourceType::Skill), Some(project), false).unwrap();
        assert!(listed.iter().any(|s| s.name == "my-skill" && !s.global));

        let updated = update_source_with_roots(
            SourceType::Skill,
            created.path.as_str(),
            "my-skill",
            "now does a different thing",
            "step three",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap();
        assert_eq!(updated.description, "now does a different thing");
        assert_eq!(updated.name, "my-skill");

        delete_source(SourceType::Skill, created.path.as_str()).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn create_rejects_duplicate_name() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().to_str().unwrap();

        create_source(
            SourceType::Skill,
            "dup",
            "d",
            "c",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap();
        let err = create_source(
            SourceType::Skill,
            "dup",
            "d",
            "c",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("already exists"));
    }

    #[test]
    fn project_scope_requires_project_dir() {
        let err = create_source(
            SourceType::Skill,
            "x",
            "d",
            "c",
            false,
            None,
            HashMap::new(),
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("projectDir"));
    }

    #[test]
    fn export_then_import_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let project_a = tmp.path().join("a");
        let project_b = tmp.path().join("b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();

        create_source(
            SourceType::Skill,
            "portable",
            "describes itself",
            "body goes here",
            false,
            Some(project_a.to_str().unwrap()),
            HashMap::new(),
        )
        .unwrap();

        let portable_dir = project_a.join(".agents").join("skills").join("portable");
        let (json, filename) =
            export_source(SourceType::Skill, portable_dir.to_str().unwrap()).unwrap();
        assert_eq!(filename, "portable.skill.json");

        let imported = import_sources(&json, false, Some(project_b.to_str().unwrap())).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].name, "portable");
        assert_eq!(imported[0].description, "describes itself");
        assert_eq!(imported[0].content, "body goes here");
    }

    #[test]
    fn export_allows_discovered_read_only_skill() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let claude_skill_dir = project.join(".claude").join("skills").join("portable");
        std::fs::create_dir_all(&claude_skill_dir).unwrap();
        std::fs::write(
            claude_skill_dir.join("SKILL.md"),
            build_skill_md(
                "portable",
                "describes itself",
                "body goes here",
                &HashMap::new(),
            ),
        )
        .unwrap();

        let listed = list_sources(
            Some(SourceType::Skill),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();
        let exported_skill = listed
            .iter()
            .find(|skill| skill.name == "portable")
            .expect("expected listed skill");

        let (json, filename) =
            export_source(SourceType::Skill, exported_skill.path.as_str()).unwrap();
        assert_eq!(filename, "portable.skill.json");
        assert!(json.contains("\"name\": \"portable\""));
    }

    #[test]
    fn update_allows_discovered_read_only_skill() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let claude_skill_dir = project.join(".claude").join("skills").join("portable");
        std::fs::create_dir_all(&claude_skill_dir).unwrap();
        std::fs::write(
            claude_skill_dir.join("SKILL.md"),
            build_skill_md(
                "portable",
                "describes itself",
                "body goes here",
                &HashMap::new(),
            ),
        )
        .unwrap();

        let updated = update_source_with_roots(
            SourceType::Skill,
            claude_skill_dir.to_str().unwrap(),
            "portable",
            "updated description",
            "updated body",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap();

        assert_eq!(updated.name, "portable");
        assert_eq!(updated.description, "updated description");
        assert_eq!(updated.content, "updated body");

        let raw = std::fs::read_to_string(claude_skill_dir.join("SKILL.md")).unwrap();
        assert!(raw.contains("description: 'updated description'"));
        assert!(raw.contains("updated body"));
    }

    #[test]
    fn import_collision_appends_suffix() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().to_str().unwrap();

        create_source(
            SourceType::Skill,
            "busy",
            "d",
            "c",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap();

        let payload = serde_json::json!({
            "version": 1,
            "type": "skill",
            "name": "busy",
            "description": "d",
            "content": "c",
        })
        .to_string();
        let imported = import_sources(&payload, false, Some(project)).unwrap();
        assert_eq!(imported[0].name, "busy-imported");
    }

    #[test]
    fn update_rejects_nonexistent_source() {
        let tmp = TempDir::new().unwrap();
        let missing_dir = tmp
            .path()
            .join(".goose")
            .join("skills")
            .join("no-such-skill");
        let err = update_source_with_roots(
            SourceType::Skill,
            missing_dir.to_str().unwrap(),
            "no-such-skill",
            "d",
            "c",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("not found"));
    }

    #[test]
    fn delete_rejects_nonexistent_source() {
        let tmp = TempDir::new().unwrap();
        let missing_dir = tmp
            .path()
            .join(".goose")
            .join("skills")
            .join("no-such-skill");
        let err = delete_source(SourceType::Skill, missing_dir.to_str().unwrap()).unwrap_err();
        assert!(format!("{:?}", err).contains("not found"));
    }

    #[test]
    fn list_sources_lists_builtin_skills() {
        let listed = list_sources(Some(SourceType::BuiltinSkill), None, false).unwrap();
        let builtin = listed
            .iter()
            .find(|source| source.name == "goose-doc-guide")
            .expect("expected goose-doc-guide builtin skill");

        assert_eq!(builtin.source_type, SourceType::BuiltinSkill);
        assert!(builtin.global);
        assert_eq!(builtin.path, "builtin://skills/goose-doc-guide");
        assert!(builtin.supporting_files.is_empty());
        assert!(!builtin.content.is_empty());
    }

    #[test]
    fn list_skill_excludes_builtin_skills() {
        let listed = list_sources(Some(SourceType::Skill), None, false).unwrap();
        assert!(!listed
            .iter()
            .any(|source| source.source_type == SourceType::BuiltinSkill));
    }

    #[test]
    fn filesystem_skill_suppresses_same_named_builtin() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let skill_dir = project
            .join(".agents")
            .join("skills")
            .join("goose-doc-guide");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            build_skill_md(
                "goose-doc-guide",
                "project override",
                "Use project docs",
                &HashMap::new(),
            ),
        )
        .unwrap();

        let builtins = list_sources(
            Some(SourceType::BuiltinSkill),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();
        assert!(!builtins
            .iter()
            .any(|source| source.name == "goose-doc-guide"));

        let skills = list_sources(
            Some(SourceType::Skill),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();
        let project_skill = skills
            .iter()
            .find(|source| source.name == "goose-doc-guide")
            .expect("expected project skill");
        assert_eq!(project_skill.source_type, SourceType::Skill);
        assert_eq!(project_skill.description, "project override");
    }

    #[test]
    fn rejects_unsupported_source_type_for_mutation() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().to_str().unwrap();

        let err = create_source(
            SourceType::BuiltinSkill,
            "x",
            "d",
            "c",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = update_source_with_roots(
            SourceType::BuiltinSkill,
            "builtin://skills/x",
            "x",
            "d",
            "c",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = update_source_with_roots(
            SourceType::Recipe,
            "x",
            "x",
            "d",
            "c",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = delete_source(SourceType::BuiltinSkill, "builtin://skills/x").unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = delete_source(SourceType::Subrecipe, "x").unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let listed = list_sources(Some(SourceType::BuiltinSkill), Some(project), false).unwrap();
        assert!(listed
            .iter()
            .any(|source| source.source_type == SourceType::BuiltinSkill));

        let err = list_sources(Some(SourceType::Recipe), Some(project), false).unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = export_source(SourceType::BuiltinSkill, "builtin://skills/x").unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let err = export_source(SourceType::Recipe, "x").unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));

        let payload = serde_json::json!({
            "version": 1,
            "type": "builtinSkill",
            "name": "x",
            "description": "d",
            "content": "c",
        })
        .to_string();
        let err = import_sources(&payload, false, Some(project)).unwrap_err();
        assert!(format!("{:?}", err).contains("not supported"));
    }

    #[test]
    fn update_derives_name_from_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().to_str().unwrap();

        create_source(
            SourceType::Skill,
            "my-dir",
            "orig",
            "body",
            false,
            Some(project),
            HashMap::new(),
        )
        .unwrap();

        let skill_dir = tmp.path().join(".agents").join("skills").join("my-dir");
        let updated = update_source_with_roots(
            SourceType::Skill,
            skill_dir.to_str().unwrap(),
            "my-dir",
            "new description",
            "new body",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap();
        // Name is derived from the frontmatter written by create_source
        assert_eq!(updated.name, "my-dir");
    }

    #[test]
    fn list_sources_reads_project_agents_skills() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join(".agents").join("skills").join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            build_skill_md("test-skill", "from agents", "Body", &HashMap::new()),
        )
        .unwrap();

        let listed = list_sources(
            Some(SourceType::Skill),
            Some(tmp.path().to_str().unwrap()),
            false,
        )
        .unwrap();
        let skill = listed
            .iter()
            .find(|source| source.name == "test-skill" && !source.global)
            .unwrap();
        assert!(skill.path.contains(".agents/skills"));
        assert_eq!(skill.description, "from agents");
    }

    #[test]
    fn project_sources_prefer_agents_directory_over_legacy_goose() {
        let tmp = TempDir::new().unwrap();
        let agents_skill_dir = tmp
            .path()
            .join(".agents")
            .join("skills")
            .join("shared-skill");
        let legacy_skill_dir = tmp
            .path()
            .join(".goose")
            .join("skills")
            .join("shared-skill");
        std::fs::create_dir_all(&agents_skill_dir).unwrap();
        std::fs::create_dir_all(&legacy_skill_dir).unwrap();
        std::fs::write(
            agents_skill_dir.join("SKILL.md"),
            build_skill_md("shared-skill", "preferred", "Agents", &HashMap::new()),
        )
        .unwrap();
        std::fs::write(
            legacy_skill_dir.join("SKILL.md"),
            build_skill_md("shared-skill", "legacy", "Goose", &HashMap::new()),
        )
        .unwrap();

        let listed = list_sources(
            Some(SourceType::Skill),
            Some(tmp.path().to_str().unwrap()),
            false,
        )
        .unwrap();
        let matching: Vec<_> = listed
            .iter()
            .filter(|source| source.name == "shared-skill" && !source.global)
            .collect();
        assert_eq!(matching.len(), 1);
        assert!(matching[0].path.contains(".agents/skills"));
        assert_eq!(matching[0].description, "preferred");

        let exported = export_source(SourceType::Skill, matching[0].path.as_str()).unwrap();
        assert!(exported.0.contains("preferred"));
    }

    #[test]
    fn list_agent_sources_includes_review_checks_with_kind_check() {
        let tmp = TempDir::new().unwrap();
        let checks_dir = tmp.path().join(".agents").join("checks");
        std::fs::create_dir_all(&checks_dir).unwrap();
        std::fs::write(
            checks_dir.join("perf.md"),
            "---\nname: perf\ndescription: Flag perf regressions\nmodel: claude-sonnet-4\nturn-limit: 40\ntools: [Read, Grep]\nseverity-default: high\n---\nLook for N+1 queries.",
        )
        .unwrap();

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(tmp.path().to_str().unwrap()),
            false,
        )
        .unwrap();

        let check = listed
            .iter()
            .find(|s| s.name == "perf")
            .expect("perf check should appear in Agent listing");
        assert_eq!(check.source_type, SourceType::Agent);
        assert_eq!(
            check.properties.get("kind").and_then(|v| v.as_str()),
            Some("check")
        );
        assert_eq!(
            check.properties.get("model").and_then(|v| v.as_str()),
            Some("claude-sonnet-4")
        );
        assert_eq!(
            check.properties.get("turnLimit").and_then(|v| v.as_u64()),
            Some(40)
        );
        assert_eq!(
            check
                .properties
                .get("severityDefault")
                .and_then(|v| v.as_str()),
            Some("high")
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_agent_sources_rejects_symlinked_review_check_outside_project() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let checks_dir = project.join(".agents").join("checks");
        std::fs::create_dir_all(&checks_dir).unwrap();

        let private_check = tmp.path().join("private").join("secret-check.md");
        std::fs::create_dir_all(private_check.parent().unwrap()).unwrap();
        std::fs::write(
            &private_check,
            "---\nname: secret-check\ndescription: hidden\n---\nTOP_SECRET_CHECK",
        )
        .unwrap();
        symlink(&private_check, checks_dir.join("secret-check.md")).unwrap();

        let listed = list_sources(
            Some(SourceType::Agent),
            Some(project.to_str().unwrap()),
            false,
        )
        .unwrap();

        assert!(!listed.iter().any(|source| {
            source.name == "secret-check" && source.content.contains("TOP_SECRET_CHECK")
        }));
    }

    #[test]
    fn update_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path();
        let escaped_dir = project.join(".goose").join("escaped");
        std::fs::create_dir_all(&escaped_dir).unwrap();
        std::fs::write(
            escaped_dir.join("SKILL.md"),
            "---\nname: escaped\ndescription: escaped\n---\ncontent",
        )
        .unwrap();

        let attempted_escape = project.join(".goose").join("escaped");
        let err = update_source_with_roots(
            SourceType::Skill,
            attempted_escape.to_str().unwrap(),
            "escaped",
            "new description",
            "new content",
            UpdateSourceOptions {
                properties: Some(HashMap::new()),
                additional_roots: &[],
            },
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("not found"));
    }
}
