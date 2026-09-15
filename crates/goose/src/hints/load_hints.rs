use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::config::paths::Paths;
use crate::conversation::message::{Message, MessageContent, MessageMetadata};
use crate::hints::import_files::read_referenced_files;

pub const GOOSE_HINTS_FILENAME: &str = ".goosehints";
pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";

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

#[derive(Default)]
pub struct SubdirectoryHintTracker {
    loaded_dirs: HashSet<PathBuf>,
    pending_dirs: Vec<PathBuf>,
    hints_filenames: Vec<String>,
}

impl SubdirectoryHintTracker {
    pub fn new() -> Self {
        Self {
            loaded_dirs: HashSet::new(),
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

    pub fn mark_loaded(&mut self, dir: PathBuf) {
        self.loaded_dirs.insert(dir);
    }

    pub fn load_new_hints(&mut self, working_dir: &Path) -> Vec<(PathBuf, String)> {
        let pending = std::mem::take(&mut self.pending_dirs);
        if pending.is_empty() {
            return Vec::new();
        }

        let Ok(working_dir) = working_dir.canonicalize() else {
            return Vec::new();
        };

        let mut results = Vec::new();
        for dir in pending {
            let Ok(dir) = dir.canonicalize() else {
                continue;
            };
            if !dir.starts_with(&working_dir) || dir == working_dir {
                continue;
            }
            if self.loaded_dirs.contains(&dir) {
                continue;
            }
            if let Some(content) =
                load_hints_from_directory(&dir, &working_dir, &self.hints_filenames)
            {
                results.push((dir.clone(), content));
            }
            self.loaded_dirs.insert(dir);
        }
        results
    }
}

const HINT_CARRIER_OPERATION: &str = "subdir_hints";
const HINT_CARRIER_DIR_NOTE: &str = "dir";

/// Agent-only messages announcing hints from subdirectories that tool calls
/// touched and no agent-visible carrier has announced yet.
pub fn pending_hint_carriers(messages: &[Message], working_dir: &Path) -> Vec<Message> {
    let mut tracker = SubdirectoryHintTracker::new();
    for message in messages {
        if message.is_agent_visible() {
            if let Some(dir) = hint_carrier_dir(message) {
                tracker.mark_loaded(dir);
            }
        }
        for content in &message.content {
            if let MessageContent::ToolRequest(request) = content {
                if let Ok(tool_call) = &request.tool_call {
                    tracker.record_tool_arguments(&tool_call.arguments, working_dir);
                }
            }
        }
    }
    tracker
        .load_new_hints(working_dir)
        .into_iter()
        .map(|(dir, content)| hint_carrier(&dir, &content))
        .collect()
}

pub(crate) fn hint_carrier(dir: &Path, content: &str) -> Message {
    let mut metadata = MessageMetadata::agent_only();
    metadata.set_operation_note(
        HINT_CARRIER_OPERATION,
        HINT_CARRIER_DIR_NOTE,
        serde_json::Value::String(dir.to_string_lossy().into_owned()),
    );
    Message::user()
        .with_text(format!(
            "<subdirectory-hints>\n{content}\n</subdirectory-hints>"
        ))
        .with_metadata(metadata)
}

pub fn hint_carrier_dir(message: &Message) -> Option<PathBuf> {
    message
        .metadata
        .operation_note(HINT_CARRIER_OPERATION, HINT_CARRIER_DIR_NOTE)?
        .as_str()
        .map(PathBuf::from)
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

    let mut contents = Vec::new();
    for dir in &directories {
        for hints_filename in hints_filenames {
            let hints_path = dir.join(hints_filename);
            if hints_path.is_file() {
                let mut visited = HashSet::new();
                let expanded = read_referenced_files(
                    &hints_path,
                    import_boundary,
                    &mut visited,
                    0,
                    &gitignore,
                );
                if !expanded.is_empty() {
                    contents.push(expanded);
                }
            }
        }
    }

    if contents.is_empty() {
        None
    } else {
        Some(format!(
            "### Subdirectory Hints ({})\n{}",
            directory.display(),
            contents.join("\n")
        ))
    }
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
    let mut global_hints_contents = Vec::with_capacity(hints_filenames.len());
    let mut local_hints_contents = Vec::with_capacity(hints_filenames.len());

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

    for global_hints_path in &global_hints_paths {
        if global_hints_path.is_file() {
            let mut visited = HashSet::new();
            let hints_dir = global_hints_path.parent().unwrap();
            let global_ignore_patterns = GitignoreBuilder::new(hints_dir)
                .build()
                .unwrap_or_else(|_| Gitignore::empty());
            let expanded_content = read_referenced_files(
                global_hints_path,
                hints_dir,
                &mut visited,
                0,
                &global_ignore_patterns,
            );
            if !expanded_content.is_empty() {
                global_hints_contents.push(expanded_content);
            }
        }
    }
    let git_root = find_git_root(cwd);
    let local_directories = get_local_directories(git_root, cwd);

    let import_boundary = git_root.unwrap_or(cwd);

    for directory in &local_directories {
        for hints_filename in hints_filenames {
            let hints_path = directory.join(hints_filename);
            if hints_path.is_file() {
                let mut visited = HashSet::new();
                let expanded_content = read_referenced_files(
                    &hints_path,
                    import_boundary,
                    &mut visited,
                    0,
                    ignore_patterns,
                );
                if !expanded_content.is_empty() {
                    local_hints_contents.push(expanded_content);
                }
            }
        }
    }

    let mut hints = String::new();
    if !global_hints_contents.is_empty() {
        hints.push_str("\n### Global Hints\nThese are my global goose hints.\n");
        hints.push_str(&global_hints_contents.join("\n"));
    }

    if !local_hints_contents.is_empty() {
        if !hints.is_empty() {
            hints.push_str("\n\n");
        }
        hints.push_str(
            "### Project Hints\nThese are hints for working on the project in this directory.\n",
        );
        hints.push_str(&local_hints_contents.join("\n"));
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
        let hints = tracker.load_new_hints(&wd);
        assert!(hints.is_empty());
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
        let hints = tracker.load_new_hints(&wd);
        assert!(hints.is_empty());
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
        let _ = tracker.load_new_hints(&wd);
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
        let hints = tracker.load_new_hints(&project_root);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].0.ends_with("nested"));
        assert!(hints[0].1.contains("nested subdirectory hints"));
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
        let hints = tracker.load_new_hints(&project_root);
        assert_eq!(hints.len(), 1);

        tracker.record_tool_arguments(&Some(args), &project_root);
        let hints = tracker.load_new_hints(&project_root);
        assert!(hints.is_empty());
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

        assert!(tracker.load_new_hints(&project_root).is_empty());
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

        assert!(tracker.load_new_hints(&project_root).is_empty());
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
        let hints = tracker.load_new_hints(&project_root);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].1.contains("real hints"));

        let real_args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "real/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(real_args), &project_root);
        assert!(tracker.load_new_hints(&project_root).is_empty());
        assert_eq!(tracker.loaded_dirs.len(), 1);
    }

    #[test]
    fn tracker_retries_directory_after_parent_is_created() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("project");
        fs::create_dir_all(&project_root).unwrap();

        let mut tracker = SubdirectoryHintTracker::new();
        let args: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"path": "future/file.rs"}"#).unwrap();
        tracker.record_tool_arguments(&Some(args.clone()), &project_root);
        assert!(tracker.load_new_hints(&project_root).is_empty());
        assert!(tracker.loaded_dirs.is_empty());

        let future = project_root.join("future");
        fs::create_dir_all(&future).unwrap();
        fs::write(future.join(GOOSE_HINTS_FILENAME), "future hints").unwrap();
        tracker.record_tool_arguments(&Some(args), &project_root);

        let hints = tracker.load_new_hints(&project_root);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].1.contains("future hints"));
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
