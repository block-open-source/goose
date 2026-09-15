use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::config::paths::Paths;
use crate::hints::import_files::{
    read_referenced_files_with_limit_and_input_budget, HintInputBudget, MAX_HINT_OUTPUT_BYTES,
};
use crate::utils::sanitize_unicode_tags;

pub const GOOSE_HINTS_FILENAME: &str = ".goosehints";
pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";
const GLOBAL_HINTS_HEADER: &str = "\n### Global Hints\nThese are my global goose hints.\n";
const PROJECT_HINTS_HEADER: &str =
    "### Project Hints\nThese are hints for working on the project in this directory.\n";
const HINT_SEPARATOR: &str = "\n\n";

struct HintLoadState {
    input_budget: HintInputBudget,
    loaded_hint_files: HashSet<PathBuf>,
}

impl HintLoadState {
    fn new() -> Self {
        Self {
            input_budget: HintInputBudget::new(),
            loaded_hint_files: HashSet::new(),
        }
    }
}

pub fn get_context_filenames() -> Vec<String> {
    use crate::config::Config;

    Config::global()
        .get_param::<Vec<String>>("CONTEXT_FILE_NAMES")
        .unwrap_or_else(|_| {
            vec![
                GOOSE_HINTS_FILENAME.to_string(),
                AGENTS_MD_FILENAME.to_string(),
            ]
        })
}

pub struct SubdirectoryHintTracker {
    loaded_dirs: Vec<PathBuf>,
    loaded_dir_set: HashSet<PathBuf>,
    emitted_hints: HashMap<PathBuf, String>,
    pending_dirs: Vec<PathBuf>,
    hints_filenames: Vec<String>,
}

impl Default for SubdirectoryHintTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SubdirectoryHintTracker {
    pub fn new() -> Self {
        Self {
            loaded_dirs: Vec::new(),
            loaded_dir_set: HashSet::new(),
            emitted_hints: HashMap::new(),
            pending_dirs: Vec::new(),
            hints_filenames: get_context_filenames(),
        }
    }

    pub fn record_tool_arguments(
        &mut self,
        arguments: &Option<serde_json::Map<String, serde_json::Value>>,
        working_dir: &Path,
    ) {
        let args = match arguments.as_ref() {
            Some(a) => a,
            None => return,
        };

        if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
            if let Some(dir) = resolve_to_parent_dir(path_str, working_dir) {
                self.pending_dirs.push(dir);
            }
        }

        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            for token in shell_words::split(cmd).unwrap_or_default() {
                if token.starts_with('-') {
                    continue;
                }
                if token.contains(std::path::MAIN_SEPARATOR) || token.contains('.') {
                    if let Some(dir) = resolve_to_parent_dir(&token, working_dir) {
                        self.pending_dirs.push(dir);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn load_snapshot(&mut self, working_dir: &Path) -> String {
        self.load_snapshot_with_limit_and_hook(working_dir, MAX_HINT_OUTPUT_BYTES, || {})
    }

    pub(crate) fn load_prompt_snapshot(
        &mut self,
        working_dir: &Path,
        output_limit: usize,
    ) -> String {
        self.load_prompt_snapshot_with_hook(working_dir, output_limit, || {})
    }

    #[cfg(test)]
    fn load_hints(&mut self, working_dir: &Path) -> String {
        self.load_snapshot(working_dir)
    }

    pub(crate) fn load_prompt_snapshot_with_hook(
        &mut self,
        working_dir: &Path,
        output_limit: usize,
        after_top_level_read: impl FnOnce(),
    ) -> String {
        self.load_snapshot_with_limit_and_hook(working_dir, output_limit, after_top_level_read)
    }

    fn load_snapshot_with_limit_and_hook(
        &mut self,
        working_dir: &Path,
        output_limit: usize,
        after_top_level_read: impl FnOnce(),
    ) -> String {
        let pending = std::mem::take(&mut self.pending_dirs);
        self.hints_filenames = get_context_filenames();
        let ignore_patterns = build_gitignore(working_dir);
        let mut load_state = HintLoadState::new();
        let mut snapshot = load_hint_files_with_state(
            working_dir,
            &self.hints_filenames,
            &ignore_patterns,
            output_limit,
            &mut load_state,
        );
        after_top_level_read();
        let Ok(canonical_working_dir) = working_dir.canonicalize() else {
            return snapshot;
        };

        for dir in pending {
            let Ok(dir) = dir.canonicalize() else {
                continue;
            };
            if !dir.starts_with(&canonical_working_dir) || dir == canonical_working_dir {
                continue;
            }
            if !self.loaded_dir_set.insert(dir.clone()) {
                continue;
            }
            self.loaded_dirs.push(dir);
        }

        for dir in &self.loaded_dirs {
            let separator = if snapshot.is_empty() {
                ""
            } else {
                HINT_SEPARATOR
            };
            let Some(directory_output_limit) = output_limit
                .checked_sub(snapshot.len())
                .and_then(|remaining| remaining.checked_sub(separator.len()))
            else {
                continue;
            };
            if let Some(content) = load_hints_from_directory(
                dir,
                &canonical_working_dir,
                &self.hints_filenames,
                directory_output_limit,
                &mut load_state,
            ) {
                snapshot.push_str(separator);
                snapshot.push_str(&content);
            }
        }
        snapshot
    }

    /// Returns changed subdirectory extras. An empty value retracts the content previously
    /// emitted for that key when the current root snapshot no longer leaves room for it.
    pub fn load_new_hints(&mut self, working_dir: &Path) -> Vec<(String, String)> {
        let pending = std::mem::take(&mut self.pending_dirs);
        self.hints_filenames = get_context_filenames();
        let mut load_state = HintLoadState::new();
        let top_level_output_bytes = load_hint_files_with_state(
            working_dir,
            &self.hints_filenames,
            &build_gitignore(working_dir),
            MAX_HINT_OUTPUT_BYTES,
            &mut load_state,
        )
        .len();
        let Ok(canonical_working_dir) = working_dir.canonicalize() else {
            return self
                .loaded_dirs
                .iter()
                .filter_map(|dir| {
                    self.emitted_hints
                        .remove(dir)
                        .map(|_| (format!("subdir_hints:{}", dir.display()), String::new()))
                })
                .collect();
        };

        let mut attempted_dirs = HashSet::new();
        for dir in pending {
            let Ok(dir) = dir.canonicalize() else {
                continue;
            };
            if !dir.starts_with(&canonical_working_dir) || dir == canonical_working_dir {
                continue;
            }
            if !attempted_dirs.insert(dir.clone()) {
                continue;
            }
            if self.loaded_dir_set.insert(dir.clone()) {
                self.loaded_dirs.push(dir.clone());
            }
        }

        let mut incremental_output_bytes = 0;
        let mut next_emitted_hints = HashMap::new();
        for dir in &self.loaded_dirs {
            let root_separator_len = if top_level_output_bytes > 0 {
                HINT_SEPARATOR.len()
            } else {
                0
            };
            let incremental_separator_len = if incremental_output_bytes > 0 {
                HINT_SEPARATOR.len()
            } else {
                0
            };
            let Some(output_limit) = MAX_HINT_OUTPUT_BYTES
                .checked_sub(top_level_output_bytes)
                .and_then(|remaining| remaining.checked_sub(root_separator_len))
                .and_then(|remaining| remaining.checked_sub(incremental_output_bytes))
                .and_then(|remaining| remaining.checked_sub(incremental_separator_len))
            else {
                continue;
            };
            if let Some(content) = load_hints_from_directory(
                dir,
                &canonical_working_dir,
                &self.hints_filenames,
                output_limit,
                &mut load_state,
            ) {
                incremental_output_bytes += incremental_separator_len + content.len();
                next_emitted_hints.insert(dir.clone(), content);
            }
        }

        let mut results = Vec::new();
        for dir in &self.loaded_dirs {
            let previous = self.emitted_hints.get(dir);
            let current = next_emitted_hints.get(dir);
            if previous != current {
                results.push((
                    format!("subdir_hints:{}", dir.display()),
                    current.cloned().unwrap_or_default(),
                ));
            }
        }
        self.emitted_hints = next_emitted_hints;
        results
    }
}

fn resolve_to_parent_dir(token: &str, working_dir: &Path) -> Option<PathBuf> {
    let path = Path::new(token);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };
    resolved.parent().map(|d| d.to_path_buf())
}

fn load_hints_from_directory(
    directory: &Path,
    working_dir: &Path,
    hints_filenames: &[String],
    output_limit: usize,
    load_state: &mut HintLoadState,
) -> Option<String> {
    if !directory.is_dir() || !directory.is_absolute() {
        return None;
    }

    if !directory.starts_with(working_dir) || directory == working_dir {
        return None;
    }

    let git_root = find_git_root(working_dir);
    let import_boundary = git_root.unwrap_or(working_dir);
    let gitignore = Gitignore::empty();

    let mut directories: Vec<PathBuf> = directory
        .ancestors()
        .take_while(|d| d.starts_with(working_dir) && *d != working_dir)
        .map(|d| d.to_path_buf())
        .collect();
    directories.reverse();

    let header = subdirectory_hints_header(directory);
    let mut output = String::new();
    let mut has_hints = false;
    for dir in &directories {
        for hints_filename in hints_filenames {
            let hints_path = dir.join(hints_filename);
            if hints_path.is_file() {
                let framing = if has_hints { "\n" } else { &header };
                if append_hint_file(
                    &mut output,
                    framing,
                    &hints_path,
                    import_boundary,
                    &gitignore,
                    output_limit,
                    load_state,
                ) {
                    has_hints = true;
                }
            }
        }
    }

    has_hints.then_some(output)
}

fn subdirectory_hints_header(directory: &Path) -> String {
    sanitize_unicode_tags(&format!(
        "### Subdirectory Hints ({})\n",
        directory.display()
    ))
}

fn append_hint_file(
    output: &mut String,
    framing: &str,
    hints_path: &Path,
    import_boundary: &Path,
    ignore_patterns: &Gitignore,
    output_limit: usize,
    load_state: &mut HintLoadState,
) -> bool {
    if load_state.loaded_hint_files.contains(hints_path) {
        return false;
    }
    let Some(used_with_framing) = output.len().checked_add(framing.len()) else {
        return false;
    };
    let Some(available) = output_limit.checked_sub(used_with_framing) else {
        return false;
    };

    let mut visited = HashSet::new();
    let expanded = read_referenced_files_with_limit_and_input_budget(
        hints_path,
        import_boundary,
        &mut visited,
        0,
        ignore_patterns,
        available,
        &mut load_state.input_budget,
    );
    if expanded.is_empty() {
        return false;
    }

    output.push_str(framing);
    output.push_str(&expanded);
    load_state
        .loaded_hint_files
        .insert(hints_path.to_path_buf());
    true
}

fn find_git_root(start_dir: &Path) -> Option<&Path> {
    let mut check_dir = start_dir;

    loop {
        if check_dir.join(".git").exists() {
            return Some(check_dir);
        }
        if let Some(parent) = check_dir.parent() {
            check_dir = parent;
        } else {
            break;
        }
    }

    None
}

fn get_local_directories(git_root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    match git_root {
        Some(git_root) => {
            let mut directories = Vec::new();
            let mut current_dir = cwd;

            loop {
                directories.push(current_dir.to_path_buf());
                if current_dir == git_root {
                    break;
                }
                if let Some(parent) = current_dir.parent() {
                    current_dir = parent;
                } else {
                    break;
                }
            }
            directories.reverse();
            directories
        }
        None => vec![cwd.to_path_buf()],
    }
}

/// Build a `Gitignore` that includes `.gitignore` files from the git root
/// down to `cwd`, matching git's hierarchical ignore semantics. When there
/// is no git root, only `cwd/.gitignore` is loaded.
pub fn build_gitignore(cwd: &Path) -> Gitignore {
    let git_root = find_git_root(cwd);
    let directories = get_local_directories(git_root, cwd);

    let mut builder = GitignoreBuilder::new(cwd);
    for dir in &directories {
        let gitignore_path = dir.join(".gitignore");
        if gitignore_path.is_file() {
            builder.add(&gitignore_path);
        }
    }
    builder.build().unwrap_or_else(|_| {
        GitignoreBuilder::new(cwd)
            .build()
            .expect("Failed to build default gitignore")
    })
}

pub fn load_hint_files(
    cwd: &Path,
    hints_filenames: &[String],
    ignore_patterns: &Gitignore,
) -> String {
    load_hint_files_with_limit(cwd, hints_filenames, ignore_patterns, MAX_HINT_OUTPUT_BYTES)
}

pub(crate) fn load_hint_files_with_limit(
    cwd: &Path,
    hints_filenames: &[String],
    ignore_patterns: &Gitignore,
    output_limit: usize,
) -> String {
    load_hint_files_with_state(
        cwd,
        hints_filenames,
        ignore_patterns,
        output_limit,
        &mut HintLoadState::new(),
    )
}

fn load_hint_files_with_state(
    cwd: &Path,
    hints_filenames: &[String],
    ignore_patterns: &Gitignore,
    output_limit: usize,
    load_state: &mut HintLoadState,
) -> String {
    let mut global_hints_paths: Vec<PathBuf> = hints_filenames
        .iter()
        .map(|name| Paths::in_config_dir(name))
        .collect();
    if hints_filenames
        .iter()
        .any(|name| name == AGENTS_MD_FILENAME)
    {
        global_hints_paths.push(Paths::in_agents_home_dir(AGENTS_MD_FILENAME));
    }

    let mut hints = String::new();
    let mut has_global_hints = false;
    for global_hints_path in &global_hints_paths {
        if global_hints_path.is_file() {
            let hints_dir = global_hints_path.parent().unwrap();
            let global_ignore_patterns = GitignoreBuilder::new(hints_dir)
                .build()
                .unwrap_or_else(|_| Gitignore::empty());
            let framing = if has_global_hints {
                "\n"
            } else {
                GLOBAL_HINTS_HEADER
            };
            if append_hint_file(
                &mut hints,
                framing,
                global_hints_path,
                hints_dir,
                &global_ignore_patterns,
                output_limit,
                load_state,
            ) {
                has_global_hints = true;
            }
        }
    }
    let git_root = find_git_root(cwd);
    let local_directories = get_local_directories(git_root, cwd);

    let import_boundary = git_root.unwrap_or(cwd);

    let mut has_local_hints = false;
    for directory in &local_directories {
        for hints_filename in hints_filenames {
            let hints_path = directory.join(hints_filename);
            if hints_path.is_file() {
                let framing = if has_local_hints {
                    "\n"
                } else if has_global_hints {
                    "\n\n### Project Hints\nThese are hints for working on the project in this directory.\n"
                } else {
                    PROJECT_HINTS_HEADER
                };
                if append_hint_file(
                    &mut hints,
                    framing,
                    &hints_path,
                    import_boundary,
                    ignore_patterns,
                    output_limit,
                    load_state,
                ) {
                    has_local_hints = true;
                }
            }
        }
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use ignore::gitignore::GitignoreBuilder;
    use std::fs;
    use tempfile::TempDir;

    fn create_dummy_gitignore() -> Gitignore {
        let temp_dir = tempfile::tempdir().expect("failed to create tempdir");
        let builder = GitignoreBuilder::new(temp_dir.path());
        builder.build().expect("failed to build gitignore")
    }

    #[test]
    fn test_goosehints_when_present() {
        let dir = TempDir::new().unwrap();

        fs::write(dir.path().join(GOOSE_HINTS_FILENAME), "Test hint content").unwrap();
        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(dir.path(), &[GOOSE_HINTS_FILENAME.to_string()], &gitignore);

        assert!(hints.contains("Test hint content"));
    }

    #[test]
    fn oversized_top_level_hint_is_rejected_without_partial_instructions() {
        let dir = TempDir::new().unwrap();
        let filename = "LOUPE_894_OVERSIZED.md";
        fs::write(
            dir.path().join(filename),
            "x".repeat(MAX_HINT_OUTPUT_BYTES + 1),
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(dir.path(), &[filename.to_string()], &gitignore);

        assert!(hints.is_empty());
    }

    #[test]
    fn top_level_hint_and_framing_fit_exact_output_limit() {
        let dir = TempDir::new().unwrap();
        let filename = "LOUPE_894_EXACT.md";
        let content = "x".repeat(MAX_HINT_OUTPUT_BYTES - PROJECT_HINTS_HEADER.len());
        fs::write(dir.path().join(filename), &content).unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(dir.path(), &[filename.to_string()], &gitignore);

        assert_eq!(hints.len(), MAX_HINT_OUTPUT_BYTES);
        assert!(hints.starts_with(PROJECT_HINTS_HEADER));
        assert!(hints.ends_with(&content));
    }

    #[test]
    fn reference_expansion_cannot_exceed_aggregate_output_limit() {
        let dir = TempDir::new().unwrap();
        let filename = "LOUPE_894_REFERENCE.md";
        let included = "expanded policy".repeat(16);
        fs::write(dir.path().join("policy.md"), &included).unwrap();
        fs::write(
            dir.path().join(filename),
            "keep this\n@policy.md\nkeep that",
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let mut output = String::new();
        let mut load_state = HintLoadState::new();
        let appended = append_hint_file(
            &mut output,
            "header\n",
            &dir.path().join(filename),
            dir.path(),
            &gitignore,
            64,
            &mut load_state,
        );

        assert!(appended);
        assert!(output.len() <= 64);
        assert!(output.contains("@policy.md"));
        assert!(!output.contains("expanded policy"));
        assert!(output.ends_with("keep that"));
    }

    #[test]
    fn multiple_top_level_hints_share_one_output_limit() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("first.md"), "A1".repeat(300 * 1024)).unwrap();
        fs::write(dir.path().join("second.md"), "B2".repeat(300 * 1024)).unwrap();
        fs::write(dir.path().join("third.md"), "third hint").unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            dir.path(),
            &[
                "first.md".to_string(),
                "second.md".to_string(),
                "third.md".to_string(),
            ],
            &gitignore,
        );

        assert!(hints.len() <= MAX_HINT_OUTPUT_BYTES);
        assert_eq!(hints.matches("A1").count(), 300 * 1024);
        assert!(!hints.contains("B2"));
        assert!(hints.ends_with("third hint"));
    }

    #[test]
    #[serial_test::serial]
    fn top_level_hint_files_share_one_raw_input_budget() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([(
            "GOOSE_PATH_ROOT",
            Some(config_root.path().to_str().unwrap()),
        )]);
        let dir = TempDir::new().unwrap();
        let tag_only_half_budget = "\u{E0001}".repeat(MAX_HINT_OUTPUT_BYTES / 8);
        fs::write(dir.path().join("first.md"), &tag_only_half_budget).unwrap();
        fs::write(dir.path().join("second.md"), &tag_only_half_budget).unwrap();
        fs::write(dir.path().join("third.md"), "MUST_NOT_BE_READ").unwrap();

        let hints = load_hint_files(
            dir.path(),
            &[
                "first.md".to_string(),
                "second.md".to_string(),
                "third.md".to_string(),
            ],
            &create_dummy_gitignore(),
        );

        assert!(!hints.contains("MUST_NOT_BE_READ"));
    }

    #[test]
    #[serial_test::serial]
    fn test_global_agents_md_in_agents_home() {
        let root = TempDir::new().unwrap();
        std::env::set_var("GOOSE_PATH_ROOT", root.path());

        let agents_home = root.path().join(".agents");
        fs::create_dir_all(&agents_home).unwrap();
        fs::write(
            agents_home.join(AGENTS_MD_FILENAME),
            "Global agents home instructions",
        )
        .unwrap();

        let project = TempDir::new().unwrap();
        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            project.path(),
            &[
                GOOSE_HINTS_FILENAME.to_string(),
                AGENTS_MD_FILENAME.to_string(),
            ],
            &gitignore,
        );

        std::env::remove_var("GOOSE_PATH_ROOT");

        assert!(hints.contains("Global Hints"));
        assert!(hints.contains("Global agents home instructions"));
    }

    #[test]
    #[serial_test::serial]
    fn global_and_local_hints_share_one_output_limit() {
        let root = TempDir::new().unwrap();
        let filename = "LOUPE_894_GLOBAL_LOCAL.md";
        std::env::set_var("GOOSE_PATH_ROOT", root.path());
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config").join(filename),
            "G1".repeat(300 * 1024),
        )
        .unwrap();

        let project = TempDir::new().unwrap();
        fs::write(project.path().join(filename), "L2".repeat(300 * 1024)).unwrap();
        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(project.path(), &[filename.to_string()], &gitignore);

        std::env::remove_var("GOOSE_PATH_ROOT");

        assert!(hints.len() <= MAX_HINT_OUTPUT_BYTES);
        assert!(hints.contains("Global Hints"));
        assert!(hints.contains("G1"));
        assert!(!hints.contains("Project Hints"));
        assert!(!hints.contains("L2"));
    }

    #[test]
    #[serial_test::serial]
    fn test_global_agents_md_imports_not_filtered_by_project_gitignore() {
        let root = TempDir::new().unwrap();
        std::env::set_var("GOOSE_PATH_ROOT", root.path());

        let agents_home = root.path().join(".agents");
        fs::create_dir_all(&agents_home).unwrap();
        fs::write(agents_home.join("policy.md"), "Imported policy content").unwrap();
        fs::write(
            agents_home.join(AGENTS_MD_FILENAME),
            "Global header\n@policy.md\n",
        )
        .unwrap();

        let project = TempDir::new().unwrap();
        let mut builder = GitignoreBuilder::new(project.path());
        builder.add_line(None, "*.md").unwrap();
        let gitignore = builder.build().unwrap();

        let hints = load_hint_files(
            project.path(),
            &[
                GOOSE_HINTS_FILENAME.to_string(),
                AGENTS_MD_FILENAME.to_string(),
            ],
            &gitignore,
        );

        std::env::remove_var("GOOSE_PATH_ROOT");

        assert!(hints.contains("Imported policy content"));
    }

    #[test]
    #[serial_test::serial]
    fn test_global_agents_md_skipped_when_not_in_context_file_names() {
        let root = TempDir::new().unwrap();
        std::env::set_var("GOOSE_PATH_ROOT", root.path());

        let agents_home = root.path().join(".agents");
        fs::create_dir_all(&agents_home).unwrap();
        fs::write(
            agents_home.join(AGENTS_MD_FILENAME),
            "Global agents home instructions",
        )
        .unwrap();

        let project = TempDir::new().unwrap();
        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            project.path(),
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        std::env::remove_var("GOOSE_PATH_ROOT");

        assert!(!hints.contains("Global agents home instructions"));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_loads_global_hints_when_working_directory_is_missing() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        fs::create_dir(config_root.path().join("config")).unwrap();
        fs::write(
            config_root.path().join("config").join(GOOSE_HINTS_FILENAME),
            "GLOBAL_HINT_AFTER_PROJECT_REMOVAL",
        )
        .unwrap();
        let missing_working_dir = config_root.path().join("removed-project");

        let snapshot = SubdirectoryHintTracker::new().load_snapshot(&missing_working_dir);

        assert!(snapshot.contains("GLOBAL_HINT_AFTER_PROJECT_REMOVAL"));
        assert!(snapshot.len() <= MAX_HINT_OUTPUT_BYTES);
    }

    #[test]
    fn test_goosehints_when_missing() {
        let dir = TempDir::new().unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(dir.path(), &[GOOSE_HINTS_FILENAME.to_string()], &gitignore);

        assert!(!hints.contains("Project Hints"));
    }

    #[test]
    fn test_goosehints_multiple_filenames() {
        let dir = TempDir::new().unwrap();

        fs::write(
            dir.path().join("CLAUDE.md"),
            "Custom hints file content from CLAUDE.md",
        )
        .unwrap();
        fs::write(
            dir.path().join(GOOSE_HINTS_FILENAME),
            "Custom hints file content from .goosehints",
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            dir.path(),
            &["CLAUDE.md".to_string(), GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(hints.contains("Custom hints file content from CLAUDE.md"));
        assert!(hints.contains("Custom hints file content from .goosehints"));
    }

    #[test]
    fn test_goosehints_configurable_filename() {
        let dir = TempDir::new().unwrap();

        fs::write(dir.path().join("CLAUDE.md"), "Custom hints file content").unwrap();
        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(dir.path(), &["CLAUDE.md".to_string()], &gitignore);

        assert!(hints.contains("Custom hints file content"));
        assert!(!hints.contains(".goosehints")); // Make sure it's not loading the default
    }

    #[test]
    fn test_nested_goosehints_with_git_root() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();
        fs::write(
            project_root.join(GOOSE_HINTS_FILENAME),
            "Root hints content",
        )
        .unwrap();

        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), "Subdir hints content").unwrap();
        let current_dir = subdir.join("current_dir");
        fs::create_dir(&current_dir).unwrap();
        fs::write(
            current_dir.join(GOOSE_HINTS_FILENAME),
            "current_dir hints content",
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            &current_dir,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(
            hints.contains("Root hints content\nSubdir hints content\ncurrent_dir hints content")
        );
    }

    #[test]
    fn test_nested_goosehints_without_git_root() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        fs::write(base_dir.join(GOOSE_HINTS_FILENAME), "Base hints content").unwrap();

        let subdir = base_dir.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), "Subdir hints content").unwrap();

        let current_dir = subdir.join("current_dir");
        fs::create_dir(&current_dir).unwrap();
        fs::write(
            current_dir.join(GOOSE_HINTS_FILENAME),
            "Current dir hints content",
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            &current_dir,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        // Without .git, should only find hints in current directory
        assert!(hints.contains("Current dir hints content"));
        assert!(!hints.contains("Base hints content"));
        assert!(!hints.contains("Subdir hints content"));
    }

    #[test]
    fn test_nested_goosehints_mixed_filenames() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();
        fs::write(project_root.join("CLAUDE.md"), "Root CLAUDE.md content").unwrap();

        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(
            subdir.join(GOOSE_HINTS_FILENAME),
            "Subdir .goosehints content",
        )
        .unwrap();

        let current_dir = subdir.join("current_dir");
        fs::create_dir(&current_dir).unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            &current_dir,
            &["CLAUDE.md".to_string(), GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(hints.contains("Root CLAUDE.md content"));
        assert!(hints.contains("Subdir .goosehints content"));
    }

    #[test]
    fn test_hints_with_basic_imports() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();

        fs::write(project_root.join("README.md"), "# Project README").unwrap();
        fs::write(project_root.join("config.md"), "Configuration details").unwrap();

        let hints_content = r#"Project hints content
@README.md
@config.md
Additional instructions here."#;
        fs::write(project_root.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            project_root,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(hints.contains("Project hints content"));
        assert!(hints.contains("Additional instructions here"));

        assert!(hints.contains("--- Content from README.md ---"));
        assert!(hints.contains("# Project README"));
        assert!(hints.contains("--- End of README.md ---"));

        assert!(hints.contains("--- Content from config.md ---"));
        assert!(hints.contains("Configuration details"));
        assert!(hints.contains("--- End of config.md ---"));
    }

    #[test]
    fn test_hints_with_git_import_boundary() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();

        fs::write(project_root.join("root_file.md"), "Root file content").unwrap();
        fs::write(
            project_root.join("shared_docs.md"),
            "Shared documentation content",
        )
        .unwrap();

        let docs_dir = project_root.join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        fs::write(docs_dir.join("api.md"), "API documentation content").unwrap();

        let utils_dir = project_root.join("src").join("utils");
        fs::create_dir_all(&utils_dir).unwrap();
        fs::write(
            utils_dir.join("helpers.md"),
            "Helper utilities content @../../shared_docs.md",
        )
        .unwrap();

        let components_dir = project_root.join("src").join("components");
        fs::create_dir_all(&components_dir).unwrap();
        fs::write(components_dir.join("local_file.md"), "Local file content").unwrap();

        let outside_dir = temp_dir.path().parent().unwrap();
        fs::write(outside_dir.join("forbidden.md"), "Forbidden content").unwrap();

        let root_hints_content = r#"Project root hints
@docs/api.md
Root level instructions"#;
        fs::write(project_root.join(GOOSE_HINTS_FILENAME), root_hints_content).unwrap();

        let nested_hints_content = r#"Nested directory hints
@local_file.md
@../utils/helpers.md
@../../docs/api.md
@../../root_file.md
@../../../forbidden.md
End of nested hints"#;
        fs::write(
            components_dir.join(GOOSE_HINTS_FILENAME),
            nested_hints_content,
        )
        .unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            &components_dir,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );
        println!("======{}", hints);
        assert!(hints.contains("Project root hints"));
        assert!(hints.contains("Root level instructions"));

        assert!(hints.contains("API documentation content"));
        assert!(hints.contains("--- Content from docs/api.md ---"));

        assert!(hints.contains("Nested directory hints"));
        assert!(hints.contains("End of nested hints"));

        assert!(hints.contains("Local file content"));
        assert!(hints.contains("--- Content from local_file.md ---"));

        assert!(hints.contains("Helper utilities content"));
        assert!(hints.contains("--- Content from ../utils/helpers.md ---"));
        assert!(hints.contains("Shared documentation content"));
        assert!(hints.contains("--- Content from ../../shared_docs.md ---"));

        let api_content_count = hints.matches("API documentation content").count();
        assert_eq!(
            api_content_count, 2,
            "API content should appear twice - from root and nested hints"
        );

        assert!(hints.contains("Root file content"));
        assert!(hints.contains("--- Content from ../../root_file.md ---"));

        assert!(!hints.contains("Forbidden content"));
        assert!(hints.contains("@../../../forbidden.md"));
    }

    #[test]
    fn test_hints_without_git_import_boundary() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path();

        let current_dir = base_dir.join("current");
        fs::create_dir(&current_dir).unwrap();
        fs::write(current_dir.join("local.md"), "Local content").unwrap();

        fs::write(base_dir.join("parent.md"), "Parent content").unwrap();

        let hints_content = r#"Current directory hints
@local.md
@../parent.md
End of hints"#;
        fs::write(current_dir.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();

        let gitignore = create_dummy_gitignore();
        let hints = load_hint_files(
            &current_dir,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(hints.contains("Local content"));
        assert!(hints.contains("--- Content from local.md ---"));

        assert!(!hints.contains("Parent content"));
        assert!(hints.contains("@../parent.md"));
    }

    #[test]
    fn test_import_boundary_respects_nested_setting() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();
        fs::create_dir(project_root.join(".git")).unwrap();
        fs::write(project_root.join("root_file.md"), "Root file content").unwrap();
        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("local_file.md"), "Local file content").unwrap();
        let hints_content = r#"Subdir hints
@local_file.md
@../root_file.md
End of hints"#;
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();
        let gitignore = create_dummy_gitignore();

        let hints = load_hint_files(&subdir, &[GOOSE_HINTS_FILENAME.to_string()], &gitignore);

        assert!(hints.contains("Local file content"));
        assert!(hints.contains("--- Content from local_file.md ---"));

        assert!(hints.contains("Root file content"));
        assert!(hints.contains("--- Content from ../root_file.md ---"));
    }

    #[test]
    fn resolve_to_parent_dir_relative() {
        let wd = Path::new("/home/user/project");
        assert_eq!(
            resolve_to_parent_dir("src/main.rs", wd),
            Some(PathBuf::from("/home/user/project/src"))
        );
    }

    #[test]
    fn resolve_to_parent_dir_absolute() {
        let wd = Path::new("/home/user/project");
        assert_eq!(
            resolve_to_parent_dir("/tmp/foo.rs", wd),
            Some(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn tracker_records_path_argument() {
        let temp_dir = TempDir::new().unwrap();
        let wd = temp_dir.path().join("project");
        let src = wd.join("src");
        fs::create_dir_all(&src).unwrap();
        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "src/main.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &wd);
        let _ = tracker.load_hints(&wd);
        assert!(tracker.loaded_dirs.contains(&src.canonicalize().unwrap()));
    }

    #[test]
    fn tracker_records_command_argument() {
        let temp_dir = TempDir::new().unwrap();
        let wd = temp_dir.path().join("project");
        let nested = wd.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"command": "cat nested/doc.md"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &wd);
        let _ = tracker.load_hints(&wd);
        assert!(tracker
            .loaded_dirs
            .contains(&nested.canonicalize().unwrap()));
    }

    #[test]
    fn tracker_skips_flags_in_command() {
        let temp_dir = TempDir::new().unwrap();
        let wd = temp_dir.path().join("project");
        let src = wd.join("src");
        fs::create_dir_all(&src).unwrap();
        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"command": "grep -rn pattern src/lib.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &wd);
        let _ = tracker.load_hints(&wd);
        assert!(tracker.loaded_dirs.contains(&src.canonicalize().unwrap()));
        assert_eq!(tracker.loaded_dirs.len(), 1);
    }

    #[test]
    fn tracker_loads_subdirectory_hints() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();
        let subdir = project_root.join("nested");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(
            subdir.join(GOOSE_HINTS_FILENAME),
            "nested subdirectory hints",
        )
        .unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "nested/foo.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &project_root);
        let hints = tracker.load_hints(&project_root);
        assert!(hints.contains("### Subdirectory Hints"));
        assert!(hints.contains("nested subdirectory hints"));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_load_new_hints_remains_incremental() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join(GOOSE_HINTS_FILENAME), "nested hints").unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&arguments, project_root.path());

        let first = tracker.load_new_hints(project_root.path());
        assert_eq!(first.len(), 1);
        assert!(first[0].1.contains("nested hints"));
        assert!(tracker.load_new_hints(project_root.path()).is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn tracker_retracts_emitted_hints_when_working_directory_disappears() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let moved = temp.path().join("moved-project");
        fs::create_dir_all(project.join("nested")).unwrap();
        fs::write(
            project.join("nested").join(GOOSE_HINTS_FILENAME),
            "NESTED_HINT",
        )
        .unwrap();
        let mut tracker = SubdirectoryHintTracker::new();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&arguments, &project);
        let first = tracker.load_new_hints(&project);
        assert_eq!(first.len(), 1);
        assert!(first[0].1.contains("NESTED_HINT"));
        assert!(tracker.load_new_hints(&project).is_empty());

        fs::rename(&project, &moved).unwrap();
        assert_eq!(
            tracker.load_new_hints(&project),
            vec![(first[0].0.clone(), String::new())]
        );
        assert!(tracker.emitted_hints.is_empty());
        assert!(tracker.load_new_hints(&project).is_empty());

        fs::rename(&moved, &project).unwrap();
        assert_eq!(tracker.load_new_hints(&project), first);
        assert!(tracker.load_new_hints(&project).is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn tracker_load_new_hints_retries_content_that_was_too_large() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let hints_path = nested.join(GOOSE_HINTS_FILENAME);
        fs::write(&hints_path, "x".repeat(MAX_HINT_OUTPUT_BYTES)).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&arguments, project_root.path());
        assert!(tracker.load_new_hints(project_root.path()).is_empty());

        fs::write(&hints_path, "now admissible").unwrap();
        tracker.record_tool_arguments(&arguments, project_root.path());
        let retried = tracker.load_new_hints(project_root.path());
        assert_eq!(retried.len(), 1);
        assert!(retried[0].1.contains("now admissible"));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_load_new_hints_shares_output_budget_across_calls() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let first = project_root.path().join("first");
        let second = project_root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join(GOOSE_HINTS_FILENAME), "a".repeat(600 * 1024)).unwrap();
        let second_hints = second.join(GOOSE_HINTS_FILENAME);
        fs::write(&second_hints, "b".repeat(600 * 1024)).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let first_arguments = serde_json::json!({ "path": "first/file.rs" })
            .as_object()
            .cloned();
        let second_arguments = serde_json::json!({ "path": "second/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&first_arguments, project_root.path());
        tracker.record_tool_arguments(&second_arguments, project_root.path());

        let initial = tracker.load_new_hints(project_root.path());
        assert_eq!(initial.len(), 1);
        let first_output_len = initial[0].1.len();

        let second_header_len = subdirectory_hints_header(&second.canonicalize().unwrap()).len();
        let payload_without_separator =
            MAX_HINT_OUTPUT_BYTES - first_output_len - second_header_len;
        fs::write(&second_hints, "b".repeat(payload_without_separator)).unwrap();
        tracker.record_tool_arguments(&second_arguments, project_root.path());
        assert!(tracker.load_new_hints(project_root.path()).is_empty());

        fs::write(
            &second_hints,
            "b".repeat(payload_without_separator - HINT_SEPARATOR.len()),
        )
        .unwrap();
        tracker.record_tool_arguments(&second_arguments, project_root.path());
        let retried = tracker.load_new_hints(project_root.path());
        assert_eq!(retried.len(), 1);
        assert_eq!(
            first_output_len + HINT_SEPARATOR.len() + retried[0].1.len(),
            MAX_HINT_OUTPUT_BYTES
        );
    }

    #[test]
    #[serial_test::serial]
    fn tracker_load_new_hints_includes_top_level_hints_in_output_budget() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(
            project_root.path().join(GOOSE_HINTS_FILENAME),
            "r".repeat(600 * 1024),
        )
        .unwrap();
        let nested_hints = nested.join(GOOSE_HINTS_FILENAME);
        fs::write(&nested_hints, "n".repeat(600 * 1024)).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&arguments, project_root.path());
        assert!(tracker.load_new_hints(project_root.path()).is_empty());

        let top_level_output_len = load_hint_files(
            project_root.path(),
            &[GOOSE_HINTS_FILENAME.to_string()],
            &build_gitignore(project_root.path()),
        )
        .len();
        let nested_header_len = subdirectory_hints_header(&nested.canonicalize().unwrap()).len();
        let exact_payload_len =
            MAX_HINT_OUTPUT_BYTES - top_level_output_len - HINT_SEPARATOR.len() - nested_header_len;

        fs::write(&nested_hints, "n".repeat(exact_payload_len + 1)).unwrap();
        tracker.record_tool_arguments(&arguments, project_root.path());
        assert!(tracker.load_new_hints(project_root.path()).is_empty());

        fs::write(&nested_hints, "n".repeat(exact_payload_len)).unwrap();
        tracker.record_tool_arguments(&arguments, project_root.path());
        let retried = tracker.load_new_hints(project_root.path());
        assert_eq!(retried.len(), 1);
        assert_eq!(
            top_level_output_len + HINT_SEPARATOR.len() + retried[0].1.len(),
            MAX_HINT_OUTPUT_BYTES
        );
    }

    #[test]
    #[serial_test::serial]
    fn tracker_load_new_hints_retracts_emitted_content_when_root_grows() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join(GOOSE_HINTS_FILENAME), "n".repeat(600 * 1024)).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&arguments, project_root.path());
        let emitted = tracker.load_new_hints(project_root.path());
        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].1.len() > 600 * 1024);

        fs::write(
            project_root.path().join(GOOSE_HINTS_FILENAME),
            "r".repeat(600 * 1024),
        )
        .unwrap();
        let updates = tracker.load_new_hints(project_root.path());
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, emitted[0].0);
        assert!(updates[0].1.is_empty());
        assert!(tracker.load_new_hints(project_root.path()).is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn tracker_refreshes_context_filenames_before_building_snapshot() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"["old.md"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("old.md"), "stale nested hints").unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        std::env::set_var("CONTEXT_FILE_NAMES", r#"["new.md"]"#);
        fs::write(
            project_root.path().join("new.md"),
            "r".repeat(MAX_HINT_OUTPUT_BYTES - PROJECT_HINTS_HEADER.len()),
        )
        .unwrap();
        let args = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        tracker.record_tool_arguments(&args, project_root.path());

        let hints = tracker.load_hints(project_root.path());

        assert_eq!(hints.len(), MAX_HINT_OUTPUT_BYTES);
        assert!(!hints.contains("stale nested hints"));
        assert_eq!(tracker.hints_filenames, ["new.md".to_string()]);
    }

    #[test]
    #[serial_test::serial]
    fn tracker_aggregates_output_limit_across_subdirectories() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();
        for directory in ["first", "second", "third"] {
            fs::create_dir(project_root.join(directory)).unwrap();
        }
        fs::write(
            project_root.join("first").join(GOOSE_HINTS_FILENAME),
            "A1".repeat(300 * 1024),
        )
        .unwrap();
        fs::write(
            project_root.join("second").join(GOOSE_HINTS_FILENAME),
            "B2".repeat(300 * 1024),
        )
        .unwrap();
        fs::write(
            project_root.join("third").join(GOOSE_HINTS_FILENAME),
            "third hint",
        )
        .unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        for directory in ["first", "second", "third"] {
            let args: serde_json::Map<String, serde_json::Value> = serde_json::from_value(
                serde_json::json!({ "path": format!("{directory}/file.rs") }),
            )
            .unwrap();
            tracker.record_tool_arguments(&Some(args), &project_root);
        }

        let hints = tracker.load_hints(&project_root);
        assert!(hints.len() <= MAX_HINT_OUTPUT_BYTES);
        assert!(hints.contains("A1"));
        assert!(!hints.contains("B2"));
        assert!(hints.contains("third hint"));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_emits_ancestor_hint_file_once_before_near_budget_descendant() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let ancestor = project_root.path().join("a");
        let descendant = ancestor.join("b");
        fs::create_dir_all(&descendant).unwrap();

        const ANCESTOR_MARKER: &str = "ANCESTOR_MARKER";
        const DESCENDANT_MARKER: &str = "DESCENDANT_MARKER";
        let ancestor_payload_len = 500 * 1024;
        fs::write(
            ancestor.join(GOOSE_HINTS_FILENAME),
            format!(
                "{}{}",
                "a".repeat(ancestor_payload_len - ANCESTOR_MARKER.len()),
                ANCESTOR_MARKER
            ),
        )
        .unwrap();

        let ancestor_output_len = subdirectory_hints_header(&ancestor.canonicalize().unwrap())
            .len()
            + ancestor_payload_len;
        let descendant_header_len =
            subdirectory_hints_header(&descendant.canonicalize().unwrap()).len();
        let descendant_payload_len = MAX_HINT_OUTPUT_BYTES
            - ancestor_output_len
            - HINT_SEPARATOR.len()
            - descendant_header_len;
        fs::write(
            descendant.join(GOOSE_HINTS_FILENAME),
            format!(
                "{}{}",
                "b".repeat(descendant_payload_len - DESCENDANT_MARKER.len()),
                DESCENDANT_MARKER
            ),
        )
        .unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        for path in ["a/file.rs", "a/b/file.rs"] {
            let arguments = serde_json::json!({ "path": path }).as_object().cloned();
            tracker.record_tool_arguments(&arguments, project_root.path());
        }
        let hints = tracker.load_snapshot(project_root.path());

        assert_eq!(hints.len(), MAX_HINT_OUTPUT_BYTES);
        assert_eq!(hints.matches(ANCESTOR_MARKER).count(), 1);
        assert!(hints.contains(DESCENDANT_MARKER));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_retries_ancestor_hints_with_a_shorter_header() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project = TempDir::new().unwrap();
        let ancestor = project.path().join("a");
        let descendant = ancestor.join("deep".repeat(30));
        fs::create_dir_all(&descendant).unwrap();
        const MARKER: &str = "ANCESTOR_MARKER";
        fs::write(ancestor.join(GOOSE_HINTS_FILENAME), MARKER).unwrap();
        let root_hints = project.path().join(GOOSE_HINTS_FILENAME);
        fs::write(&root_hints, "ROOT").unwrap();
        let root_framing = load_hint_files(
            project.path(),
            &[GOOSE_HINTS_FILENAME.to_string()],
            &build_gitignore(project.path()),
        )
        .len()
            - "ROOT".len();
        let ancestor_output =
            subdirectory_hints_header(&ancestor.canonicalize().unwrap()).len() + MARKER.len();
        assert!(
            subdirectory_hints_header(&descendant.canonicalize().unwrap()).len() > ancestor_output
        );
        fs::write(
            &root_hints,
            "r".repeat(
                MAX_HINT_OUTPUT_BYTES - root_framing - HINT_SEPARATOR.len() - ancestor_output,
            ),
        )
        .unwrap();

        for incremental in [false, true] {
            let mut tracker = SubdirectoryHintTracker::new();
            for directory in [&descendant, &ancestor] {
                let arguments = serde_json::json!({ "path": directory.join("file.rs") })
                    .as_object()
                    .cloned();
                tracker.record_tool_arguments(&arguments, project.path());
            }
            if incremental {
                let extras = tracker.load_new_hints(project.path());
                assert_eq!(extras.len(), 1);
                assert_eq!(
                    extras[0].0,
                    format!(
                        "subdir_hints:{}",
                        ancestor.canonicalize().unwrap().display()
                    )
                );
                assert_eq!(extras[0].1.len(), ancestor_output);
                assert_eq!(extras[0].1.matches(MARKER).count(), 1);
                assert!(tracker.load_new_hints(project.path()).is_empty());
            } else {
                let snapshot = tracker.load_snapshot(project.path());
                assert_eq!(snapshot.len(), MAX_HINT_OUTPUT_BYTES);
                assert_eq!(snapshot.matches(MARKER).count(), 1);
            }
        }
    }

    #[test]
    fn retrying_unadmitted_hint_files_still_charges_raw_input() {
        let project = TempDir::new().unwrap();
        let malformed = project.path().join("malformed.md");
        let valid = project.path().join("valid.md");
        fs::write(&malformed, vec![0xff; MAX_HINT_OUTPUT_BYTES / 2]).unwrap();
        fs::write(&valid, "VALID_HINT").unwrap();
        let mut state = HintLoadState::new();
        let mut output = String::new();
        for path in [&malformed, &malformed, &valid] {
            assert!(!append_hint_file(
                &mut output,
                "",
                path,
                project.path(),
                &Gitignore::empty(),
                MAX_HINT_OUTPUT_BYTES,
                &mut state,
            ));
        }
        assert!(output.is_empty());
        assert!(state.loaded_hint_files.is_empty());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn tracker_preserves_lexical_scope_for_symlinked_hint_files() {
        use std::os::unix::fs::symlink;

        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = TempDir::new().unwrap();
        let first = project_root.path().join("a");
        let second = project_root.path().join("b");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(project_root.path().join("shared-hints"), "@policy.md").unwrap();
        fs::write(first.join("policy.md"), "FIRST_SCOPED_POLICY").unwrap();
        fs::write(second.join("policy.md"), "SECOND_SCOPED_POLICY").unwrap();
        symlink("../shared-hints", first.join(GOOSE_HINTS_FILENAME)).unwrap();
        symlink("../shared-hints", second.join(GOOSE_HINTS_FILENAME)).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        for path in ["a/file.rs", "b/file.rs"] {
            let arguments = serde_json::json!({ "path": path }).as_object().cloned();
            tracker.record_tool_arguments(&arguments, project_root.path());
        }
        let hints = tracker.load_snapshot(project_root.path());

        assert!(hints.contains("FIRST_SCOPED_POLICY"));
        assert!(hints.contains("SECOND_SCOPED_POLICY"));
    }

    #[test]
    #[serial_test::serial]
    fn tracker_charges_separator_between_root_and_subdirectory_hints() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project_root = tempfile::tempdir().unwrap();
        let nested = project_root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(
            project_root.path().join(GOOSE_HINTS_FILENAME),
            "r".repeat(MAX_HINT_OUTPUT_BYTES - PROJECT_HINTS_HEADER.len() - 2048),
        )
        .unwrap();
        let hints_filenames = vec![GOOSE_HINTS_FILENAME.to_string()];
        let ignore_patterns = build_gitignore(project_root.path());
        let top_level_hints =
            load_hint_files(project_root.path(), &hints_filenames, &ignore_patterns);
        let remaining = MAX_HINT_OUTPUT_BYTES - top_level_hints.len();
        let canonical_nested = nested.canonicalize().unwrap();
        let subdirectory_header =
            format!("### Subdirectory Hints ({})\n", canonical_nested.display()).len();
        let nested_hints = nested.join(GOOSE_HINTS_FILENAME);
        fs::write(
            &nested_hints,
            "n".repeat(remaining - 1 - subdirectory_header),
        )
        .unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .unwrap()
            .clone();
        tracker.record_tool_arguments(&Some(args), project_root.path());
        let snapshot = tracker.load_hints(project_root.path());
        assert_eq!(snapshot, top_level_hints);

        fs::write(
            nested_hints,
            "n".repeat(remaining - HINT_SEPARATOR.len() - subdirectory_header),
        )
        .unwrap();
        let hints = tracker.load_hints(project_root.path());
        assert_eq!(hints.len(), MAX_HINT_OUTPUT_BYTES);
        assert!(hints.contains(&top_level_hints));
    }

    #[test]
    fn subdirectory_header_is_normalized_before_budgeting() {
        let directory = Path::new("nested\u{0344}");
        let header = subdirectory_hints_header(directory);

        assert_eq!(header, "### Subdirectory Hints (nested\u{0308}\u{0301})\n");
        assert!(header.len() > format!("### Subdirectory Hints ({})\n", directory.display()).len());
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn tracker_reserves_hints_from_a_lexical_symlink_working_directory() {
        use std::os::unix::fs::symlink;

        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let repository = temp_dir.path().join("repository");
        let target = temp_dir.path().join("target");
        let nested = target.join("nested");
        fs::create_dir_all(repository.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            repository.join(GOOSE_HINTS_FILENAME),
            "r".repeat(MAX_HINT_OUTPUT_BYTES - PROJECT_HINTS_HEADER.len() - 1),
        )
        .unwrap();
        fs::write(nested.join(GOOSE_HINTS_FILENAME), "nested hint").unwrap();
        let working_dir = repository.join("linked-working-dir");
        symlink(&target, &working_dir).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        tracker.hints_filenames = vec![GOOSE_HINTS_FILENAME.to_string()];
        let args = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .unwrap()
            .clone();
        tracker.record_tool_arguments(&Some(args), &working_dir);

        let snapshot = tracker.load_hints(&working_dir);
        assert_eq!(snapshot.len(), MAX_HINT_OUTPUT_BYTES - 1);
        assert!(!snapshot.contains("nested hint"));
    }

    #[test]
    fn tracker_deduplicates_directories() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();
        let subdir = project_root.join("nested");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), "nested hints").unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "nested/foo.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args.clone()), &project_root);
        let hints = tracker.load_hints(&project_root);
        assert_eq!(hints.matches("nested hints").count(), 1);

        tracker.record_tool_arguments(&Some(args), &project_root);
        let hints = tracker.load_hints(&project_root);
        assert_eq!(hints.matches("nested hints").count(), 1);
        assert_eq!(tracker.loaded_dirs.len(), 1);
    }

    #[test]
    fn tracker_preserves_first_seen_order_with_companion_membership_set() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();
        for directory in ["second", "first", "third"] {
            let path = project_root.join(directory);
            fs::create_dir(&path).unwrap();
            fs::write(
                path.join(GOOSE_HINTS_FILENAME),
                format!("{directory} hints"),
            )
            .unwrap();
        }

        let mut tracker = SubdirectoryHintTracker::new();
        for directory in ["second", "first", "third", "second", "first"] {
            let arguments = serde_json::json!({
                "path": format!("{directory}/file.rs")
            })
            .as_object()
            .cloned();
            tracker.record_tool_arguments(&arguments, &project_root);
        }
        let hints = tracker.load_hints(&project_root);
        let expected = ["second", "first", "third"]
            .map(|directory| project_root.join(directory).canonicalize().unwrap());

        assert_eq!(tracker.loaded_dirs, expected);
        assert_eq!(tracker.loaded_dir_set.len(), expected.len());
        assert!(expected
            .iter()
            .all(|directory| tracker.loaded_dir_set.contains(directory)));
        let second = hints.find("second hints").unwrap();
        let first = hints.find("first hints").unwrap();
        let third = hints.find("third hints").unwrap();
        assert!(second < first && first < third);
    }

    #[test]
    fn tracker_rejects_parent_traversal_outside_working_directory() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("project");
        let nested = project_root.join("nested");
        let outside = temp_dir.path().join("outside");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(GOOSE_HINTS_FILENAME), "outside hints").unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "nested/../../outside/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &project_root);

        assert!(tracker.load_hints(&project_root).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn tracker_rejects_directory_symlink_outside_working_directory() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("project");
        let outside = temp_dir.path().join("outside");
        fs::create_dir_all(&project_root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(GOOSE_HINTS_FILENAME), "outside hints").unwrap();
        symlink(&outside, project_root.join("alias")).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "alias/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args), &project_root);

        let hints = tracker.load_hints(&project_root);
        assert!(!hints.contains("outside hints"));
    }

    #[cfg(unix)]
    #[test]
    fn tracker_preserves_in_boundary_symlinks_and_deduplicates_aliases() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("project");
        let real = project_root.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join(GOOSE_HINTS_FILENAME), "real hints").unwrap();
        symlink(&real, project_root.join("alias")).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let alias_args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "alias/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(alias_args), &project_root);
        let hints = tracker.load_hints(&project_root);
        assert!(hints.contains("real hints"));

        let real_args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "real/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(real_args), &project_root);
        assert_eq!(
            tracker
                .load_hints(&project_root)
                .matches("real hints")
                .count(),
            1
        );
        assert_eq!(tracker.loaded_dirs.len(), 1);
    }

    #[test]
    #[serial_test::serial]
    fn tracker_retries_directory_after_parent_is_created() {
        let config_root = TempDir::new().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("project");
        fs::create_dir_all(&project_root).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "future/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args.clone()), &project_root);
        assert!(tracker.load_hints(&project_root).is_empty());
        assert!(tracker.loaded_dirs.is_empty());

        let future = project_root.join("future");
        fs::create_dir_all(&future).unwrap();
        fs::write(future.join(GOOSE_HINTS_FILENAME), "future hints").unwrap();
        tracker.record_tool_arguments(&Some(args), &project_root);

        let hints = tracker.load_hints(&project_root);
        assert!(hints.contains("future hints"));
    }
}

#[cfg(test)]
mod gitignore_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_hints_with_gitignore_filters_referenced_files() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();
        fs::write(project_root.join("allowed.md"), "Allowed content").unwrap();
        fs::write(project_root.join("secret.env"), "SECRET_KEY=abc123").unwrap();
        fs::write(project_root.join(".gitignore"), "*.env\n").unwrap();

        let hints_content = "Project hints\n@allowed.md\n@secret.env\nEnd of hints";
        fs::write(project_root.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();

        let gitignore = build_gitignore(project_root);

        let hints = load_hint_files(
            project_root,
            &[GOOSE_HINTS_FILENAME.to_string()],
            &gitignore,
        );

        assert!(hints.contains("Allowed content"));
        assert!(!hints.contains("SECRET_KEY=abc123"));
        assert!(hints.contains("@secret.env"));
    }

    #[test]
    fn test_build_gitignore_loads_from_git_root_in_subdirectory() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();
        // Root .gitignore ignores .env files
        fs::write(project_root.join(".gitignore"), "*.env\n").unwrap();
        fs::write(project_root.join("secret.env"), "SECRET_KEY=abc123").unwrap();
        fs::write(project_root.join("allowed.md"), "Allowed content").unwrap();

        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();

        let hints_content = "Subdir hints\n@../allowed.md\n@../secret.env\nEnd of hints";
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();

        // Build gitignore from the subdirectory — should still pick up root .gitignore
        let gitignore = build_gitignore(&subdir);

        let hints = load_hint_files(&subdir, &[GOOSE_HINTS_FILENAME.to_string()], &gitignore);

        assert!(hints.contains("Allowed content"));
        assert!(!hints.contains("SECRET_KEY=abc123"));
        assert!(hints.contains("@../secret.env"));
    }

    #[test]
    fn test_build_gitignore_merges_nested_gitignores() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();

        fs::create_dir(project_root.join(".git")).unwrap();
        // Root ignores *.log
        fs::write(project_root.join(".gitignore"), "*.log\n").unwrap();

        let subdir = project_root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        // Subdir ignores *.tmp
        fs::write(subdir.join(".gitignore"), "*.tmp\n").unwrap();

        fs::write(project_root.join("debug.log"), "debug log").unwrap();
        fs::write(subdir.join("cache.tmp"), "temp data").unwrap();
        fs::write(subdir.join("readme.md"), "Readme content").unwrap();

        let hints_content = "Hints\n@../debug.log\n@cache.tmp\n@readme.md\nEnd";
        fs::write(subdir.join(GOOSE_HINTS_FILENAME), hints_content).unwrap();

        let gitignore = build_gitignore(&subdir);
        let hints = load_hint_files(&subdir, &[GOOSE_HINTS_FILENAME.to_string()], &gitignore);

        assert!(hints.contains("Readme content"));
        assert!(!hints.contains("debug log"));
        assert!(!hints.contains("temp data"));
        assert!(hints.contains("@../debug.log"));
        assert!(hints.contains("@cache.tmp"));
    }
}
