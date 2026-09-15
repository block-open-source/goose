use super::discover_skills_with_config;
use super::loaded_skill_context_with_args;
use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::ToolCallContext;
use crate::config::Config;
use async_trait::async_trait;
use goose_sdk_types::custom_requests::{SourceEntry, SourceType};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeResult, JsonObject, ListToolsResult,
    ServerCapabilities, ServerNotification, Tool,
};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub static EXTENSION_NAME: &str = "skills";

pub struct SkillsClient {
    info: InitializeResult,
    working_dir: PathBuf,
    exclude_builtin_skills: bool,
    config: &'static Config,
}

impl SkillsClient {
    pub fn new(context: PlatformExtensionContext) -> anyhow::Result<Self> {
        let working_dir = context
            .session
            .as_ref()
            .map(|s| s.working_dir.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        let info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(EXTENSION_NAME, "1.0.0").with_title("Skills"));

        Ok(Self {
            info,
            working_dir,
            exclude_builtin_skills: false,
            config: Config::global(),
        })
    }

    /// Controls whether Goose's bundled skills are exposed by this client.
    /// Bundled skills are enabled by default.
    pub fn with_builtin_skills(mut self, enabled: bool) -> Self {
        self.exclude_builtin_skills = !enabled;
        self
    }

    #[cfg(test)]
    fn with_config(mut self, config: &'static Config) -> Self {
        self.config = config;
        self
    }

    fn discover_skills(&self) -> Vec<SourceEntry> {
        discover_skills_with_config(Some(&self.working_dir), self.config)
            .into_iter()
            .filter(|skill| {
                !self.exclude_builtin_skills || skill.source_type != SourceType::BuiltinSkill
            })
            .collect()
    }

    fn discover_skills_with_details(&self) -> Vec<super::DiscoveredSkill> {
        super::discover_skills_with_details_and_config(Some(&self.working_dir), self.config)
            .into_iter()
            .filter(|skill| {
                !self.exclude_builtin_skills || skill.source_type != SourceType::BuiltinSkill
            })
            .collect()
    }
}

#[async_trait]
impl McpClientTrait for SkillsClient {
    async fn list_tools(
        &self,
        _session_id: &str,
        _next_cursor: Option<String>,
        _cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to load. Use \"skill-name/path\" to load a supporting file."
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments to provide when loading the skill."
                }
            }
        });

        let tool = Tool::new(
            "load_skill",
            "Load a skill's full content into your context so you can follow its instructions.\n\n\
             Skills are listed in your system instructions. When you need to use one, \
             load it first to get the detailed instructions.\n\n\
             Examples:\n\
             - load_skill(name: \"gdrive\") → Loads the gdrive skill instructions\n\
             - load_skill(name: \"my-skill\", args: \"the arguments for the skill\") → Loads a skill with arguments\n\
             - load_skill(name: \"my-skill/template.md\") → Loads a supporting file"
                .to_string(),
            schema.as_object().unwrap().clone(),
        );

        Ok(ListToolsResult {
            tools: vec![tool],
            next_cursor: None,
            meta: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        _ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        _cancellation_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        if name != "load_skill" {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Unknown tool: {}",
                name
            ))]));
        }

        let skill_name = arguments
            .as_ref()
            .and_then(|args| args.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if skill_name.is_empty() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Missing required parameter: name",
            )]));
        }
        let args = arguments
            .as_ref()
            .and_then(|args| args.get("args"))
            .and_then(|v| v.as_str());

        let skills = self.discover_skills_with_details();

        if let Some(skill) = skills.iter().find(|s| s.name == skill_name) {
            return match loaded_skill_context_with_args(skill, args) {
                Ok(rendered) => Ok(CallToolResult::success(vec![ContentBlock::text(rendered)])),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "Failed to parse skill arguments: {}",
                    e
                ))])),
            };
        }

        if let Some((parent_skill_name, raw_relative_path)) = skill_name.split_once('/') {
            let relative_path = raw_relative_path.replace('\\', "/");
            if let Some(skill) = skills.iter().find(|s| {
                s.name == parent_skill_name
                    && matches!(s.source_type, SourceType::Skill | SourceType::BuiltinSkill)
            }) {
                let listed_skill_dir = PathBuf::from(&skill.path);

                for file_path in &skill.supporting_files {
                    let file_path_buf = Path::new(file_path);
                    let Ok(rel) = file_path_buf.strip_prefix(&listed_skill_dir) else {
                        continue;
                    };
                    if rel.to_string_lossy().replace('\\', "/") != relative_path {
                        continue;
                    }

                    return Ok(load_supporting_file(skill, skill_name, rel));
                }

                let available: Vec<String> = skill
                    .supporting_files
                    .iter()
                    .filter_map(|f| {
                        Path::new(f)
                            .strip_prefix(&listed_skill_dir)
                            .ok()
                            .map(|r| r.to_string_lossy().replace('\\', "/"))
                    })
                    .take(10)
                    .collect();

                return Ok(if available.is_empty() {
                    CallToolResult::error(vec![ContentBlock::text(format!(
                        "Skill '{}' has no supporting files.",
                        skill.name
                    ))])
                } else {
                    CallToolResult::error(vec![ContentBlock::text(format!(
                        "File '{}' not found. Available: {}",
                        skill_name,
                        available.join(", ")
                    ))])
                });
            }
        }

        let suggestions: Vec<&str> = skills
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&skill_name.to_lowercase())
                    || skill_name.to_lowercase().contains(&s.name.to_lowercase())
            })
            .take(3)
            .map(|s| s.name.as_str())
            .collect();

        Ok(if suggestions.is_empty() {
            CallToolResult::error(vec![ContentBlock::text(format!(
                "Skill '{}' not found.",
                skill_name
            ))])
        } else {
            CallToolResult::error(vec![ContentBlock::text(format!(
                "Skill '{}' not found. Did you mean: {}?",
                skill_name,
                suggestions.join(", ")
            ))])
        })
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }

    fn get_instructions(&self) -> Option<String> {
        let sources = self.discover_skills();
        let mut skills: Vec<&SourceEntry> = sources
            .iter()
            .filter(|s| {
                s.source_type == SourceType::Skill || s.source_type == SourceType::BuiltinSkill
            })
            .collect();
        skills.sort_by(|a, b| (&a.name, &a.path).cmp(&(&b.name, &b.path)));

        if skills.is_empty() {
            return None;
        }

        let mut instructions = String::from(
            "\n\nYou have these skills at your disposal, when it is clear they can help you solve a problem or you are asked to use them:",
        );
        for skill in &skills {
            instructions.push_str(&format!("\n• {} - {}", skill.name, skill.description));
        }
        Some(instructions)
    }

    async fn subscribe(&self) -> mpsc::Receiver<ServerNotification> {
        let (_tx, rx) = mpsc::channel(1);
        rx
    }
}

pub(super) fn load_supporting_file(
    skill: &super::DiscoveredSkill,
    skill_name: &str,
    relative: &Path,
) -> CallToolResult {
    match skill.load_supporting_file(relative, skill_name) {
        Ok(content) => CallToolResult::success(vec![ContentBlock::text(content)]),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(format!(
            "Failed to read '{}': {}",
            skill_name, error
        ))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write_plugin_skill(
        project: &Path,
        plugin_name: &str,
        skill_name: &str,
        description: &str,
        body: &str,
    ) {
        let skill_dir = project
            .join(".agents/plugins")
            .join(plugin_name)
            .join("skills")
            .join(skill_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {skill_name}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    fn write_open_plugin_manifest(project: &Path, plugin_name: &str) {
        let plugin_dir = project.join(".agents/plugins").join(plugin_name);
        fs::write(
            plugin_dir.join("plugin.json"),
            format!(
                r#"{{"name":"{plugin_name}","skills":{{"paths":["./skills","./custom-skills"]}}}}"#
            ),
        )
        .unwrap();
    }

    fn test_client(project: &Path, plugin_name: &str, enabled: bool) -> SkillsClient {
        let config = Box::leak(Box::new(
            Config::new(project.join("test-config.yaml"), "goose-skills-test").unwrap(),
        ));
        let plugin_root = project.join(".agents/plugins").join(plugin_name);
        config
            .set_param(
                "plugins",
                HashMap::from([(
                    plugin_root.to_string_lossy().into_owned(),
                    HashMap::from([("enabled", enabled)]),
                )]),
            )
            .unwrap();
        let session = Arc::new(crate::session::Session {
            working_dir: project.to_path_buf(),
            ..crate::session::Session::default()
        });
        SkillsClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager: Arc::new(crate::session::SessionManager::instance()),
            scheduler: None,
            session: Some(session),
            use_login_shell_path: false,
        })
        .unwrap()
        .with_builtin_skills(false)
        .with_config(config)
    }

    fn result_text(result: &CallToolResult) -> &str {
        match &result.content[0] {
            ContentBlock::Text(text) => &text.text,
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn disabled_plugin_skill_is_not_listed_or_loadable() {
        let _guard = env_lock::lock_env([("PLUGINS", None::<&str>)]);
        let project = TempDir::new().unwrap();
        write_plugin_skill(
            project.path(),
            "disabled-plugin",
            "disabled-plugin-skill",
            "Disabled plugin metadata",
            "disabled plugin full body",
        );
        let client = test_client(project.path(), "disabled-plugin", false);

        assert!(client
            .get_instructions()
            .is_none_or(|instructions| !instructions.contains("disabled-plugin-skill")));

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let args = serde_json::from_value(serde_json::json!({
            "name": "disabled-plugin-skill"
        }))
        .unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(result.is_error.unwrap_or(false));
        assert!(!result_text(&result).contains("disabled plugin full body"));
    }

    #[tokio::test]
    async fn enabled_plugin_skill_is_listed_and_loadable() {
        let _guard = env_lock::lock_env([("PLUGINS", None::<&str>)]);
        let project = TempDir::new().unwrap();
        write_plugin_skill(
            project.path(),
            "enabled-plugin",
            "enabled-plugin-skill",
            "Enabled plugin metadata",
            "enabled plugin full body",
        );
        let custom_skill_dir = project
            .path()
            .join(".agents/plugins/enabled-plugin/custom-skills/custom-plugin-skill");
        fs::create_dir_all(&custom_skill_dir).unwrap();
        fs::write(
            custom_skill_dir.join("SKILL.md"),
            "---\nname: custom-plugin-skill\ndescription: Custom plugin metadata\n---\ncustom plugin full body",
        )
        .unwrap();
        write_open_plugin_manifest(project.path(), "enabled-plugin");
        let client = test_client(project.path(), "enabled-plugin", true);

        let instructions = client.get_instructions().unwrap();
        assert!(instructions.contains("enabled-plugin-skill"));
        assert!(instructions.contains("Enabled plugin metadata"));
        assert!(instructions.contains("custom-plugin-skill"));
        assert!(instructions.contains("Custom plugin metadata"));

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let args = serde_json::from_value(serde_json::json!({
            "name": "custom-plugin-skill"
        }))
        .unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        assert!(result_text(&result).contains("custom plugin full body"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_project_plugin_supporting_file_is_loadable() {
        let _guard = env_lock::lock_env([("PLUGINS", None::<&str>)]);
        let project = TempDir::new().unwrap();
        let external = TempDir::new().unwrap();
        write_plugin_skill(
            external.path(),
            "symlinked-plugin",
            "symlinked-skill",
            "Symlinked skill metadata",
            "symlinked skill body",
        );
        write_open_plugin_manifest(external.path(), "symlinked-plugin");
        let external_plugin = external.path().join(".agents/plugins/symlinked-plugin");
        let supporting_file = external_plugin.join("skills/symlinked-skill/guide.md");
        fs::write(&supporting_file, "Symlinked supporting guidance.").unwrap();

        let plugin_link = project.path().join(".agents/plugins/symlinked-plugin");
        fs::create_dir_all(plugin_link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&external_plugin, &plugin_link).unwrap();
        let client = test_client(project.path(), "symlinked-plugin", true);

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let args = serde_json::from_value(serde_json::json!({
            "name": "symlinked-skill/guide.md"
        }))
        .unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        assert!(result_text(&result).contains("Symlinked supporting guidance."));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nested_skill_under_linked_root_loads_only_regular_supporting_files() {
        let project = TempDir::new().unwrap();
        let working_dir = project.path().canonicalize().unwrap();
        let skill_root = working_dir.join(".agents/skills");
        fs::create_dir_all(&skill_root).unwrap();
        let external_skill = working_dir.join("external-skill");
        fs::create_dir_all(external_skill.join("nested")).unwrap();
        fs::write(
            external_skill.join("SKILL.md"),
            "---\nname: outer-skill\ndescription: Outer skill\n---\nOuter body",
        )
        .unwrap();
        fs::write(
            external_skill.join("nested/SKILL.md"),
            "---\nname: nested-skill\ndescription: Nested skill\n---\nNested body",
        )
        .unwrap();
        fs::write(external_skill.join("nested/guide.md"), "Nested guidance.").unwrap();
        let escaped = working_dir.join("escaped");
        fs::create_dir(&escaped).unwrap();
        fs::write(escaped.join("secret.md"), "outside secret").unwrap();
        std::os::unix::fs::symlink(&escaped, external_skill.join("nested/escaped")).unwrap();
        std::os::unix::fs::symlink(&external_skill, skill_root.join("linked-skill")).unwrap();

        let session = Arc::new(crate::session::Session {
            working_dir,
            ..crate::session::Session::default()
        });
        let client = SkillsClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager: Arc::new(crate::session::SessionManager::instance()),
            scheduler: None,
            session: Some(session),
            use_login_shell_path: false,
        })
        .unwrap()
        .with_builtin_skills(false);
        let ctx = ToolCallContext::new("test".to_string(), None, None);

        let guide_args = serde_json::from_value(serde_json::json!({
            "name": "nested-skill/guide.md"
        }))
        .unwrap();
        let guide = client
            .call_tool(
                &ctx,
                "load_skill",
                Some(guide_args),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(!guide.is_error.unwrap_or(false));
        assert!(result_text(&guide).contains("Nested guidance."));

        let escaped_args = serde_json::from_value(serde_json::json!({
            "name": "nested-skill/escaped/secret.md"
        }))
        .unwrap();
        let escaped = client
            .call_tool(
                &ctx,
                "load_skill",
                Some(escaped_args),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(escaped.is_error.unwrap_or(false));
        assert!(!result_text(&escaped).contains("outside secret"));
    }

    #[tokio::test]
    async fn test_load_filesystem_skill_without_builtin_skills() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().canonicalize().unwrap();
        let skill_dir = working_dir.join(".goose/skills/my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: A test skill\n---\nDo the thing.",
        )
        .unwrap();
        fs::create_dir(skill_dir.join("nested")).unwrap();
        fs::write(skill_dir.join("nested/guide.md"), "Nested guidance.").unwrap();

        let session = std::sync::Arc::new(crate::session::Session {
            working_dir,
            ..crate::session::Session::default()
        });
        let client = SkillsClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager: Arc::new(crate::session::SessionManager::instance()),
            scheduler: None,
            session: Some(session),
            use_login_shell_path: false,
        })
        .unwrap()
        .with_builtin_skills(false);

        assert!(client
            .discover_skills()
            .iter()
            .all(|skill| skill.source_type != SourceType::BuiltinSkill));

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let args: JsonObject =
            serde_json::from_value(serde_json::json!({"name": "my-skill"})).unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        let text = match &result.content[0] {
            rmcp::model::ContentBlock::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("my-skill"));
        assert!(text.contains("Do the thing"));

        let args: JsonObject =
            serde_json::from_value(serde_json::json!({"name": "my-skill/nested/guide.md"}))
                .unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(!result.is_error.unwrap_or(false));
        let text = match &result.content[0] {
            rmcp::model::ContentBlock::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        assert!(text.contains("Nested guidance."));
    }

    #[tokio::test]
    async fn test_load_skill_not_found_returns_error() {
        let client = SkillsClient::new(PlatformExtensionContext {
            extension_manager: None,
            session_manager: Arc::new(crate::session::SessionManager::instance()),
            scheduler: None,
            session: None,
            use_login_shell_path: false,
        })
        .unwrap();

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let args: JsonObject =
            serde_json::from_value(serde_json::json!({"name": "nonexistent"})).unwrap();
        let result = client
            .call_tool(&ctx, "load_skill", Some(args), CancellationToken::new())
            .await
            .unwrap();

        assert!(result.is_error.unwrap_or(false));
    }
}
