use super::*;
use crate::config::extensions::name_to_key;
use serde::Deserialize;
use serde_yaml::Mapping;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const GOOSE_CONFIG_PREFIX: &str = "goose_config:";
const CLAUDE_DESKTOP_PREFIX: &str = "claude_desktop:";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeDesktopConfig {
    #[serde(default)]
    mcp_servers: HashMap<String, ClaudeMcpServer>,
}

#[derive(Debug, Deserialize)]
struct ClaudeMcpServer {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

impl GooseAcpAgent {
    pub(super) async fn on_onboarding_import_scan(
        &self,
        req: OnboardingImportScanRequest,
    ) -> Result<OnboardingImportScanResponse, agent_client_protocol::Error> {
        let source_filter = source_filter(&req.sources);
        let mut candidates = Vec::new();

        if source_filter.contains(&OnboardingImportSourceKind::GooseConfig) {
            for path in goose_config_candidate_paths(&self.config_dir) {
                if let Some(candidate) = scan_goose_config_candidate(&path) {
                    candidates.push(candidate);
                }
            }
        }

        if source_filter.contains(&OnboardingImportSourceKind::ClaudeDesktop) {
            for path in claude_desktop_candidate_paths() {
                if let Some(candidate) = scan_claude_desktop_candidate(&path) {
                    candidates.push(candidate);
                }
            }
        }

        candidates.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        Ok(OnboardingImportScanResponse { candidates })
    }

    pub(super) async fn on_onboarding_import_apply(
        &self,
        req: OnboardingImportApplyRequest,
    ) -> Result<OnboardingImportApplyResponse, agent_client_protocol::Error> {
        let config = self.config()?;
        Ok(apply_onboarding_import_candidates(
            config,
            &self.config_dir,
            &req,
        ))
    }
}

fn apply_onboarding_import_candidates(
    config: &Config,
    target_config_dir: &Path,
    req: &OnboardingImportApplyRequest,
) -> OnboardingImportApplyResponse {
    let mut imported = OnboardingImportCounts::default();
    let mut skipped = OnboardingImportCounts::default();
    let mut warnings = Vec::new();
    let mut provider_defaults = None;

    for candidate_id in &req.candidate_ids {
        match parse_candidate_id(candidate_id) {
            Some((OnboardingImportSourceKind::GooseConfig, path)) => {
                match apply_goose_config_candidate(
                    config,
                    target_config_dir,
                    &path,
                    req.enable_imported_extensions,
                ) {
                    Ok(result) => {
                        add_counts(&mut imported, &result.imported);
                        add_counts(&mut skipped, &result.skipped);
                        warnings.extend(result.warnings);
                        if result.provider_defaults.provider_id.is_some()
                            || result.provider_defaults.model_id.is_some()
                        {
                            provider_defaults = Some(result.provider_defaults);
                        }
                    }
                    Err(error) => warnings.push(import_failure_warning(
                        OnboardingImportSourceKind::GooseConfig,
                        &path,
                        &error,
                    )),
                }
            }
            Some((OnboardingImportSourceKind::ClaudeDesktop, path)) => {
                match apply_claude_desktop_candidate(config, &path, req.enable_imported_extensions)
                {
                    Ok(result) => {
                        add_counts(&mut imported, &result.imported);
                        add_counts(&mut skipped, &result.skipped);
                        warnings.extend(result.warnings);
                    }
                    Err(error) => warnings.push(import_failure_warning(
                        OnboardingImportSourceKind::ClaudeDesktop,
                        &path,
                        &error,
                    )),
                }
            }
            None => warnings.push(format!("Skipped unknown import candidate: {candidate_id}")),
        }
    }

    OnboardingImportApplyResponse {
        imported,
        skipped,
        warnings,
        provider_defaults,
    }
}

#[derive(Default)]
struct ApplyResult {
    imported: OnboardingImportCounts,
    skipped: OnboardingImportCounts,
    warnings: Vec<String>,
    provider_defaults: DefaultsReadResponse,
}

fn source_filter(sources: &[OnboardingImportSourceKind]) -> HashSet<OnboardingImportSourceKind> {
    if sources.is_empty() {
        return [
            OnboardingImportSourceKind::GooseConfig,
            OnboardingImportSourceKind::ClaudeDesktop,
        ]
        .into_iter()
        .collect();
    }

    sources.iter().copied().collect()
}

fn add_counts(target: &mut OnboardingImportCounts, source: &OnboardingImportCounts) {
    target.providers += source.providers;
    target.extensions += source.extensions;
    target.sessions += source.sessions;
    target.skills += source.skills;
    target.projects += source.projects;
    target.preferences += source.preferences;
}

fn import_failure_warning(
    source_kind: OnboardingImportSourceKind,
    path: &Path,
    error: &anyhow::Error,
) -> String {
    let source_name = match source_kind {
        OnboardingImportSourceKind::GooseConfig => "Goose configuration",
        OnboardingImportSourceKind::ClaudeDesktop => "Claude Desktop tools",
    };
    format!(
        "Skipped {source_name} import at {}: {error}",
        path.display()
    )
}

fn goose_config_candidate_paths(config_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![config_dir.join(CONFIG_YAML_NAME)];
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".config").join("goose").join(CONFIG_YAML_NAME));
    }
    dedupe_paths(paths)
}

fn claude_desktop_candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        paths.push(config_dir.join("Claude").join("claude_desktop_config.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json"),
        );
        paths.push(
            home.join("AppData")
                .join("Roaming")
                .join("Claude")
                .join("claude_desktop_config.json"),
        );
    }
    dedupe_paths(paths)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn scan_goose_config_candidate(path: &Path) -> Option<OnboardingImportCandidate> {
    if !path.exists() {
        return None;
    }

    let mapping = read_yaml_mapping(path).ok()?;
    let mut counts = OnboardingImportCounts::default();
    let mut warnings = Vec::new();

    if mapping_contains_string(&mapping, "GOOSE_PROVIDER")
        || mapping_contains_string(&mapping, "GOOSE_MODEL")
    {
        counts.providers = 1;
    }
    counts.extensions = extension_count(&mapping);
    counts.skills = path
        .parent()
        .map(|dir| count_skill_dirs(&dir.join("skills")))
        .unwrap_or_default();

    if counts.sessions > 0 {
        warnings.push("Sessions are already shared through Goose's data store.".to_string());
    }

    Some(OnboardingImportCandidate {
        id: candidate_id(GOOSE_CONFIG_PREFIX, path),
        source_kind: OnboardingImportSourceKind::GooseConfig,
        display_name: "Existing Goose configuration".to_string(),
        path: path.to_string_lossy().to_string(),
        counts,
        warnings,
    })
}

fn scan_claude_desktop_candidate(path: &Path) -> Option<OnboardingImportCandidate> {
    if !path.exists() {
        return None;
    }

    let (servers, warnings) = read_claude_servers(path).ok()?;
    if servers.is_empty() && warnings.is_empty() {
        return None;
    }

    Some(OnboardingImportCandidate {
        id: candidate_id(CLAUDE_DESKTOP_PREFIX, path),
        source_kind: OnboardingImportSourceKind::ClaudeDesktop,
        display_name: "Claude Desktop tools".to_string(),
        path: path.to_string_lossy().to_string(),
        counts: OnboardingImportCounts {
            extensions: servers.len() as u32,
            ..Default::default()
        },
        warnings,
    })
}

fn candidate_id(prefix: &str, path: &Path) -> String {
    format!("{prefix}{}", path.to_string_lossy())
}

fn parse_candidate_id(id: &str) -> Option<(OnboardingImportSourceKind, PathBuf)> {
    if let Some(path) = id.strip_prefix(GOOSE_CONFIG_PREFIX) {
        return Some((OnboardingImportSourceKind::GooseConfig, PathBuf::from(path)));
    }
    if let Some(path) = id.strip_prefix(CLAUDE_DESKTOP_PREFIX) {
        return Some((
            OnboardingImportSourceKind::ClaudeDesktop,
            PathBuf::from(path),
        ));
    }
    None
}

fn read_yaml_mapping(path: &Path) -> anyhow::Result<Mapping> {
    let content = fs::read_to_string(path)?;
    let value: serde_yaml::Value = serde_yaml::from_str(&content)?;
    Ok(value.as_mapping().cloned().unwrap_or_default())
}

fn mapping_contains_string(mapping: &Mapping, key: &str) -> bool {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn extension_count(mapping: &Mapping) -> u32 {
    mapping
        .get(serde_yaml::Value::String("extensions".to_string()))
        .and_then(|value| value.as_mapping())
        .map(|extensions| extensions.len() as u32)
        .unwrap_or_default()
}

fn count_skill_dirs(path: &Path) -> u32 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };

    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("SKILL.md").exists())
        .count() as u32
}

fn apply_goose_config_candidate(
    target_config: &Config,
    target_config_dir: &Path,
    source_path: &Path,
    enable_imported_extensions: bool,
) -> anyhow::Result<ApplyResult> {
    let source = read_yaml_mapping(source_path)?;
    let mut result = ApplyResult::default();

    let provider = yaml_string(&source, "GOOSE_PROVIDER");
    let model = yaml_string(&source, "GOOSE_MODEL");
    if let Some(ref p) = provider {
        let m = model.clone().unwrap_or_else(|| {
            crate::config::get_provider_entry(target_config, p)
                .map(|e| e.model)
                .unwrap_or_default()
        });
        crate::config::set_active_provider(target_config, p, &m)?;
        result.provider_defaults = DefaultsReadResponse {
            provider_id: provider.clone(),
            model_id: model,
        };
        result.imported.providers = 1;
    }

    let extension_result =
        import_goose_config_extensions(target_config, &source, enable_imported_extensions)?;
    result.imported.extensions += extension_result.imported;
    result.skipped.extensions += extension_result.skipped;

    if let Some(source_dir) = source_path.parent() {
        let secrets_result = import_file_secrets(target_config, &source_dir.join("secrets.yaml"))?;
        if secrets_result > 0 && result.imported.providers == 0 {
            result.imported.providers = 1;
        }

        let skills_result = import_skill_dirs(
            &source_dir.join("skills"),
            &target_config_dir.join("skills"),
        )?;
        result.imported.skills += skills_result.imported;
        result.skipped.skills += skills_result.skipped;
    }

    result
        .warnings
        .push("Session history already lives in the Goose data store when available.".to_string());
    Ok(result)
}

fn yaml_string(mapping: &Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_string()))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Default)]
struct ImportPairCount {
    imported: u32,
    skipped: u32,
}

fn import_goose_config_extensions(
    target_config: &Config,
    source: &Mapping,
    enable_imported_extensions: bool,
) -> anyhow::Result<ImportPairCount> {
    let Some(source_extensions) = source
        .get(serde_yaml::Value::String("extensions".to_string()))
        .and_then(|value| value.as_mapping())
    else {
        return Ok(ImportPairCount::default());
    };

    let mut target_extensions = load_target_extensions(target_config)?;
    let mut counts = ImportPairCount::default();
    for (key, value) in source_extensions {
        if target_extensions.contains_key(key) {
            counts.skipped += 1;
            continue;
        }
        let mut value = value.clone();
        if !enable_imported_extensions {
            if let Some(extension) = value.as_mapping_mut() {
                extension.insert(
                    serde_yaml::Value::String("enabled".to_string()),
                    serde_yaml::Value::Bool(false),
                );
            }
        }
        target_extensions.insert(key.clone(), value);
        counts.imported += 1;
    }

    if counts.imported > 0 {
        target_config.set_param("extensions", target_extensions)?;
    }

    Ok(counts)
}

fn load_target_extensions(target_config: &Config) -> anyhow::Result<Mapping> {
    match target_config.get_param::<Mapping>("extensions") {
        Ok(extensions) => Ok(extensions),
        Err(crate::config::ConfigError::NotFound(_)) => Ok(Mapping::new()),
        Err(error) => Err(error.into()),
    }
}

fn import_file_secrets(target_config: &Config, source_path: &Path) -> anyhow::Result<u32> {
    if !source_path.exists() {
        return Ok(0);
    }

    let mapping = read_yaml_mapping(source_path)?;
    let mut updates = Vec::new();
    for (key, value) in mapping {
        let Some(key) = key.as_str() else {
            continue;
        };
        updates.push((key.to_string(), serde_json::to_value(value)?));
    }
    target_config.set_secret_values(&updates)?;
    Ok(updates.len() as u32)
}

fn import_skill_dirs(source_root: &Path, target_root: &Path) -> anyhow::Result<ImportPairCount> {
    let Ok(entries) = fs::read_dir(source_root) else {
        return Ok(ImportPairCount::default());
    };

    let mut counts = ImportPairCount::default();
    fs::create_dir_all(target_root)?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let source = entry.path();
        if !source.join("SKILL.md").exists() {
            continue;
        }
        let Some(name) = source.file_name() else {
            continue;
        };
        let target = target_root.join(name);
        if target.exists() {
            counts.skipped += 1;
            continue;
        }
        copy_dir_recursively(&source, &target)?;
        counts.imported += 1;
    }
    Ok(counts)
}

fn copy_dir_recursively(source: &Path, target: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = fs::symlink_metadata(&source_path)?.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_dir_recursively(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn apply_claude_desktop_candidate(
    target_config: &Config,
    source_path: &Path,
    enable_imported_extensions: bool,
) -> anyhow::Result<ApplyResult> {
    let (servers, mut warnings) = read_claude_servers(source_path)?;
    let mut target_extensions = load_target_extensions(target_config)?;
    let mut result = ApplyResult::default();

    for (name, server) in servers {
        let command = match server.command {
            Some(command) if !command.trim().is_empty() => command,
            _ => {
                result.skipped.extensions += 1;
                warnings.push(format!(
                    "Skipped Claude MCP server '{name}' because it has no command."
                ));
                continue;
            }
        };

        let key = serde_yaml::Value::String(name_to_key(&name));
        if target_extensions.contains_key(&key) {
            result.skipped.extensions += 1;
            continue;
        }

        let entry = crate::config::extensions::ExtensionEntry {
            enabled: enable_imported_extensions,
            config: ExtensionConfig::Stdio {
                name,
                description: "Imported from Claude Desktop".to_string(),
                cmd: command,
                args: server.args,
                envs: Envs::new(server.env),
                env_keys: Vec::new(),
                timeout: Some(crate::config::DEFAULT_EXTENSION_TIMEOUT),
                cwd: None,
                bundled: None,
                available_tools: Vec::new(),
            },
        };
        target_extensions.insert(key, serde_yaml::to_value(entry)?);
        result.imported.extensions += 1;
    }

    if result.imported.extensions > 0 {
        target_config.set_param("extensions", target_extensions)?;
    }

    result.warnings.extend(warnings);
    if !enable_imported_extensions && result.imported.extensions > 0 {
        result
            .warnings
            .push("Imported Claude Desktop MCP servers are disabled by default.".to_string());
    }
    Ok(result)
}

fn read_claude_servers(
    path: &Path,
) -> anyhow::Result<(HashMap<String, ClaudeMcpServer>, Vec<String>)> {
    let content = fs::read_to_string(path)?;
    let config: ClaudeDesktopConfig = serde_json::from_str(&content)?;
    let mut servers = HashMap::new();
    let mut warnings = Vec::new();

    for (name, server) in config.mcp_servers {
        if server
            .command
            .as_deref()
            .is_some_and(|command| !command.trim().is_empty())
        {
            servers.insert(name, server);
        } else {
            warnings.push(format!("Claude MCP server '{name}' has no command."));
        }
    }

    Ok((servers, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_claude_desktop_counts_valid_servers() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("claude_desktop_config.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "github": { "command": "npx", "args": ["github-mcp"] },
                "broken": { "args": ["missing-command"] }
              }
            }"#,
        )
        .unwrap();

        let candidate = scan_claude_desktop_candidate(&path).unwrap();
        assert_eq!(candidate.counts.extensions, 1);
        assert_eq!(candidate.warnings.len(), 1);
    }

    #[test]
    fn apply_onboarding_imports_continues_after_candidate_failure() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let missing_goose_config = source.path().join("missing-config.yaml");
        let claude_config = source.path().join("claude_desktop_config.json");
        fs::write(
            &claude_config,
            r#"{
              "mcpServers": {
                "github": { "command": "npx", "args": ["github-mcp"] }
              }
            }"#,
        )
        .unwrap();
        let target_config = Config::new_with_file_secrets(
            target.path().join(CONFIG_YAML_NAME),
            target.path().join("secrets.yaml"),
        )
        .unwrap();
        let req = OnboardingImportApplyRequest {
            candidate_ids: vec![
                candidate_id(GOOSE_CONFIG_PREFIX, &missing_goose_config),
                candidate_id(CLAUDE_DESKTOP_PREFIX, &claude_config),
            ],
            enable_imported_extensions: false,
        };

        let response = apply_onboarding_import_candidates(&target_config, target.path(), &req);

        assert_eq!(response.imported.extensions, 1);
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Skipped Goose configuration import at ")));
        let extensions = target_config.get_param::<Mapping>("extensions").unwrap();
        assert!(extensions.contains_key(serde_yaml::Value::String(name_to_key("github"))));
    }

    #[test]
    fn goose_import_disables_new_extensions_when_requested() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let source_config = source.path().join(CONFIG_YAML_NAME);
        fs::write(
            &source_config,
            r#"
extensions:
  payload:
    enabled: true
    type: stdio
    name: payload
    description: imported command
    cmd: /tmp/payload
    args: []
"#,
        )
        .unwrap();
        let target_config = Config::new_with_file_secrets(
            target.path().join(CONFIG_YAML_NAME),
            target.path().join("secrets.yaml"),
        )
        .unwrap();
        let req = OnboardingImportApplyRequest {
            candidate_ids: vec![candidate_id(GOOSE_CONFIG_PREFIX, &source_config)],
            enable_imported_extensions: false,
        };

        let response = apply_onboarding_import_candidates(&target_config, target.path(), &req);

        assert_eq!(response.imported.extensions, 1);
        let extensions = target_config.get_param::<Mapping>("extensions").unwrap();
        let entry = extensions
            .get(serde_yaml::Value::String("payload".to_string()))
            .and_then(serde_yaml::Value::as_mapping)
            .unwrap();
        assert_eq!(
            entry
                .get(serde_yaml::Value::String("enabled".to_string()))
                .and_then(serde_yaml::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn apply_goose_config_imports_defaults_extensions_and_skills() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let source_config = source.path().join(CONFIG_YAML_NAME);
        fs::write(
            &source_config,
            r#"
GOOSE_PROVIDER: openai
GOOSE_MODEL: gpt-5.1
extensions:
  github:
    enabled: true
    type: stdio
    name: github
    description: GitHub
    cmd: npx
    args: ["github-mcp"]
"#,
        )
        .unwrap();
        let skill_dir = source.path().join("skills").join("reviewer");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Reviewer").unwrap();

        let target_config = Config::new_with_file_secrets(
            target.path().join(CONFIG_YAML_NAME),
            target.path().join("secrets.yaml"),
        )
        .unwrap();

        let result =
            apply_goose_config_candidate(&target_config, target.path(), &source_config, true)
                .unwrap();

        assert_eq!(result.imported.providers, 1);
        assert_eq!(result.imported.extensions, 1);
        assert_eq!(result.imported.skills, 1);
        assert_eq!(target_config.get_goose_provider().unwrap(), "openai");
        assert!(target.path().join("skills").join("reviewer").exists());
        let extensions = target_config.get_param::<Mapping>("extensions").unwrap();
        let entry = extensions
            .get(serde_yaml::Value::String("github".to_string()))
            .and_then(serde_yaml::Value::as_mapping)
            .unwrap();
        assert_eq!(
            entry
                .get(serde_yaml::Value::String("enabled".to_string()))
                .and_then(serde_yaml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn apply_goose_config_model_only_skips_provider_activation() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let source_config = source.path().join(CONFIG_YAML_NAME);
        fs::write(&source_config, "GOOSE_MODEL: gpt-5.1\n").unwrap();

        let target_config = Config::new_with_file_secrets(
            target.path().join(CONFIG_YAML_NAME),
            target.path().join("secrets.yaml"),
        )
        .unwrap();

        let result =
            apply_goose_config_candidate(&target_config, target.path(), &source_config, false)
                .unwrap();

        assert_eq!(result.imported.providers, 0);
        assert!(target_config.get_goose_provider().is_err());
    }

    #[test]
    fn goose_import_does_not_change_colliding_target_extension() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let source_config = source.path().join(CONFIG_YAML_NAME);
        fs::write(
            &source_config,
            r#"
extensions:
  payload:
    enabled: true
    type: stdio
    name: payload
    description: imported command
    cmd: /tmp/imported
    args: []
"#,
        )
        .unwrap();
        let target_config_path = target.path().join(CONFIG_YAML_NAME);
        fs::write(
            &target_config_path,
            r#"
extensions:
  payload:
    enabled: true
    type: stdio
    name: payload
    description: existing command
    cmd: /tmp/existing
    args: []
"#,
        )
        .unwrap();
        let target_config =
            Config::new_with_file_secrets(target_config_path, target.path().join("secrets.yaml"))
                .unwrap();
        let req = OnboardingImportApplyRequest {
            candidate_ids: vec![candidate_id(GOOSE_CONFIG_PREFIX, &source_config)],
            enable_imported_extensions: false,
        };

        let response = apply_onboarding_import_candidates(&target_config, target.path(), &req);

        assert_eq!(response.imported.extensions, 0);
        assert_eq!(response.skipped.extensions, 1);
        let extensions = target_config.get_param::<Mapping>("extensions").unwrap();
        let entry = extensions
            .get(serde_yaml::Value::String("payload".to_string()))
            .and_then(serde_yaml::Value::as_mapping)
            .unwrap();
        assert_eq!(
            entry
                .get(serde_yaml::Value::String("enabled".to_string()))
                .and_then(serde_yaml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            entry
                .get(serde_yaml::Value::String("cmd".to_string()))
                .and_then(serde_yaml::Value::as_str),
            Some("/tmp/existing")
        );
    }

    #[cfg(unix)]
    #[test]
    fn import_skill_dirs_skips_symlink_cycles() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let skill_dir = source.path().join("reviewer");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Reviewer").unwrap();
        std::os::unix::fs::symlink(&skill_dir, skill_dir.join("loop")).unwrap();

        let result = import_skill_dirs(source.path(), target.path()).unwrap();

        assert_eq!(result.imported, 1);
        assert!(target.path().join("reviewer").join("SKILL.md").exists());
        assert!(!target.path().join("reviewer").join("loop").exists());
    }
}
