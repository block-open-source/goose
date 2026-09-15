//! Goose-specific inference request preparation.

#[cfg(feature = "code-mode")]
use crate::agents::ExtensionManager;
use crate::agents::PromptManager;
use crate::config::GooseMode;
use crate::hints::{SubdirectoryHintTracker, MAX_HINT_OUTPUT_BYTES};
use crate::session::Session;
use crate::tool_inspection::ToolInspectionManager;
use anyhow::Result;
use async_trait::async_trait;
use goose_agent::inference::{InferenceRequestPreparer, PreparedInferenceRequest};
use goose_agent::operation::{messages_since_kickoff, InferenceInput};
use goose_providers::conversation::message::{Message, MessageContent};
use goose_providers::conversation::Conversation;
#[cfg(feature = "code-mode")]
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) fn reconstructed_hint_snapshot(
    conversation: &Conversation,
    working_dir: &std::path::Path,
) -> String {
    reconstructed_hint_snapshot_with_hook(conversation, working_dir, || {})
}

fn reconstructed_hint_snapshot_with_hook(
    conversation: &Conversation,
    working_dir: &std::path::Path,
    after_top_level_read: impl FnOnce(),
) -> String {
    let mut hints = SubdirectoryHintTracker::new();
    for message in conversation.messages() {
        for content in &message.content {
            if let MessageContent::ToolRequest(request) = content {
                if let Ok(tool_call) = &request.tool_call {
                    hints.record_tool_arguments(&tool_call.arguments, working_dir);
                }
            }
        }
    }
    hints.load_prompt_snapshot_with_hook(working_dir, MAX_HINT_OUTPUT_BYTES, after_top_level_read)
}

pub struct GooseInferenceRequestPreparer<'a> {
    #[cfg(feature = "code-mode")]
    pub(crate) extension_manager: Arc<ExtensionManager>,
    pub(crate) goose_mode: &'a Mutex<GooseMode>,
    pub(crate) prompt_manager: &'a Mutex<PromptManager>,
    pub(crate) tool_inspection_manager: &'a ToolInspectionManager,
    pub(crate) context_limit: usize,
}

#[async_trait]
impl InferenceRequestPreparer<Session> for GooseInferenceRequestPreparer<'_> {
    async fn prepare(
        &self,
        session: &Session,
        conversation: &Conversation,
        input: InferenceInput,
    ) -> Result<PreparedInferenceRequest> {
        #[cfg(feature = "code-mode")]
        let code_execution_mode = self
            .extension_manager
            .is_extension_enabled(
                crate::agents::platform_extensions::code_execution::EXTENSION_NAME,
            )
            .await;
        #[cfg(not(feature = "code-mode"))]
        let code_execution_mode = false;

        let goose_mode = *self.goose_mode.lock().await;
        if goose_mode == GooseMode::SmartApprove {
            self.tool_inspection_manager
                .apply_tool_annotations(&input.tools);
        }
        let tools =
            crate::agents::reply_parts::prepare_inference_tools(input.tools, code_execution_mode);
        let mut prompt_manager = self.prompt_manager.lock().await;
        let hint_snapshot = reconstructed_hint_snapshot(conversation, &session.working_dir);
        let system_prompt = prompt_manager.build_system_prompt_from_snapshot(
            input.prompt_parts,
            goose_mode,
            hint_snapshot,
        );
        let turn = messages_since_kickoff(conversation)?;
        let turn_start = turn
            .first()
            .and_then(|message| chrono::DateTime::from_timestamp(message.created, 0))
            .map(|timestamp| timestamp.with_timezone(&chrono::Local))
            .unwrap_or_else(chrono::Local::now);
        let last = turn
            .iter()
            .rev()
            .find(|message| message.is_turn_context())
            .map(Message::as_concat_text);
        let context_limit = Some(self.context_limit);
        let additional_messages = crate::agents::moim::turn_context_event(
            &session.working_dir,
            context_limit,
            input.moim_parts,
            turn_start,
        )
        .filter(|event| Some(event.as_concat_text()) != last)
        .into_iter()
        .collect();
        Ok(PreparedInferenceRequest {
            system_prompt,
            tools,
            additional_messages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hints::{GOOSE_HINTS_FILENAME, MAX_HINT_OUTPUT_BYTES};
    use rmcp::model::CallToolRequestParams;
    use std::fs;

    #[test]
    #[serial_test::serial]
    fn reconstructed_hint_snapshot_is_stable_across_file_growth() {
        let config_root = tempfile::tempdir().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project = tempfile::tempdir().unwrap();
        let nested = project.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let root_hints = project.path().join(GOOSE_HINTS_FILENAME);
        fs::write(&root_hints, "ROOT_V1").unwrap();
        fs::write(nested.join(GOOSE_HINTS_FILENAME), "NESTED_MARKER").unwrap();

        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .unwrap()
            .clone();
        let conversation = Conversation::new_unvalidated([Message::assistant().with_tool_request(
            "read-nested",
            Ok(CallToolRequestParams::new("read_file").with_arguments(arguments)),
        )]);

        let snapshot = reconstructed_hint_snapshot_with_hook(&conversation, project.path(), || {
            fs::write(&root_hints, format!("{}ROOT_V2", "v".repeat(700 * 1024))).unwrap()
        });
        fs::write(&root_hints, "ROOT_V3").unwrap();

        let prompt = PromptManager::new().build_system_prompt_from_snapshot(
            Vec::new(),
            GooseMode::Auto,
            snapshot.clone(),
        );

        assert!(snapshot.len() <= MAX_HINT_OUTPUT_BYTES);
        assert!(prompt.contains("ROOT_V1"));
        assert!(prompt.contains("NESTED_MARKER"));
        assert!(!prompt.contains("ROOT_V2"));
        assert!(!prompt.contains("ROOT_V3"));
    }

    #[test]
    #[serial_test::serial]
    fn both_loops_drop_project_hints_when_working_directory_disappears() {
        let config_root = tempfile::tempdir().unwrap();
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
            "GLOBAL_HINT",
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(project.join("nested")).unwrap();
        fs::write(project.join(GOOSE_HINTS_FILENAME), "ROOT_HINT").unwrap();
        fs::write(
            project.join("nested").join(GOOSE_HINTS_FILENAME),
            "NESTED_HINT",
        )
        .unwrap();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        let conversation = Conversation::new_unvalidated([Message::assistant().with_tool_request(
            "read-nested",
            Ok(CallToolRequestParams::new("read_file").with_arguments(arguments.clone().unwrap())),
        )]);
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
        let mut legacy = PromptManager::with_timestamp(timestamp);
        legacy.set_system_prompt_override("base".to_string());
        legacy.record_tool_arguments(&arguments, &project);
        assert!(legacy
            .builder_with_fresh_hints(&project, GooseMode::Auto)
            .build()
            .contains("NESTED_HINT"));

        fs::rename(&project, temp.path().join("moved-project")).unwrap();
        assert!(legacy.load_subdirectory_hints(&project));
        let legacy_prompt = legacy
            .builder_with_fresh_hints(&project, GooseMode::Auto)
            .build();
        let mut state_machine = PromptManager::with_timestamp(timestamp);
        state_machine.set_system_prompt_override("base".to_string());
        let prompt = state_machine.build_system_prompt_from_snapshot(
            Vec::new(),
            GooseMode::Auto,
            reconstructed_hint_snapshot(&conversation, &project),
        );
        assert_eq!(legacy_prompt, prompt);
        assert!(prompt.contains("GLOBAL_HINT"));
        assert!(!prompt.contains("ROOT_HINT"));
        assert!(!prompt.contains("NESTED_HINT"));
    }

    #[test]
    #[serial_test::serial]
    fn both_loops_bound_one_snapshot_and_deduplicate_ancestor_hints() {
        let config_root = tempfile::tempdir().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("a/b")).unwrap();
        let root_hints = project.path().join(GOOSE_HINTS_FILENAME);
        fs::write(
            &root_hints,
            format!("ROOT_MARKER{}", "r".repeat(600 * 1024)),
        )
        .unwrap();
        fs::write(
            project.path().join("a").join(GOOSE_HINTS_FILENAME),
            format!("ANCESTOR_MARKER{}", "a".repeat(300 * 1024)),
        )
        .unwrap();
        fs::write(
            project.path().join("a/b").join(GOOSE_HINTS_FILENAME),
            format!("DESCENDANT_MARKER{}", "b".repeat(300 * 1024)),
        )
        .unwrap();
        let arguments = ["a/file.rs", "a/b/file.rs"].map(|path| {
            serde_json::json!({ "path": path })
                .as_object()
                .unwrap()
                .clone()
        });
        let conversation = Conversation::new_unvalidated(
            arguments
                .iter()
                .enumerate()
                .map(|(index, args)| {
                    Message::assistant().with_tool_request(
                        format!("read-{index}"),
                        Ok(CallToolRequestParams::new("read_file").with_arguments(args.clone())),
                    )
                })
                .collect::<Vec<_>>(),
        );
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
        let mut legacy = PromptManager::with_timestamp(timestamp);
        legacy.set_system_prompt_override("base".to_string());
        for args in &arguments {
            legacy.record_tool_arguments(&Some(args.clone()), project.path());
        }

        for root in [None, Some("SHORT_ROOT")] {
            if let Some(content) = root {
                fs::write(&root_hints, content).unwrap();
            }
            let snapshot = reconstructed_hint_snapshot(&conversation, project.path());
            assert!(snapshot.len() <= MAX_HINT_OUTPUT_BYTES);
            assert_eq!(snapshot.matches("ANCESTOR_MARKER").count(), 1);
            assert_eq!(snapshot.contains("DESCENDANT_MARKER"), root.is_some());
            let mut state_machine = PromptManager::with_timestamp(timestamp);
            state_machine.set_system_prompt_override("base".to_string());
            let state_machine_prompt = state_machine.build_system_prompt_from_snapshot(
                Vec::new(),
                GooseMode::Auto,
                snapshot,
            );
            let legacy_prompt = legacy
                .builder_with_fresh_hints(project.path(), GooseMode::Auto)
                .build();
            assert_eq!(legacy_prompt, state_machine_prompt);
            let output = legacy_prompt
                .strip_prefix("base\n\n# Additional Instructions:\n\n")
                .unwrap();
            assert!(output.len() <= MAX_HINT_OUTPUT_BYTES);
        }
    }

    #[test]
    #[serial_test::serial]
    fn legacy_and_state_machine_use_the_same_snapshot_without_mutating_caller_extras() {
        let config_root = tempfile::tempdir().unwrap();
        let _guard = env_lock::lock_env([
            (
                "GOOSE_PATH_ROOT",
                Some(config_root.path().to_str().unwrap()),
            ),
            ("CONTEXT_FILE_NAMES", Some(r#"[".goosehints"]"#)),
        ]);
        let project = tempfile::tempdir().unwrap();
        let nested = project.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(project.path().join(GOOSE_HINTS_FILENAME), "ROOT_HINT").unwrap();
        fs::write(nested.join(GOOSE_HINTS_FILENAME), "NESTED_HINT").unwrap();
        let arguments = serde_json::json!({ "path": "nested/file.rs" })
            .as_object()
            .cloned();
        let conversation = Conversation::new_unvalidated([Message::assistant().with_tool_request(
            "read-nested",
            Ok(CallToolRequestParams::new("read_file").with_arguments(arguments.clone().unwrap())),
        )]);
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();

        let mut legacy = PromptManager::with_timestamp(timestamp);
        legacy.set_system_prompt_override("base".to_string());
        legacy.add_system_prompt_extra("caller".to_string(), "CALLER_EXTRA".to_string());
        legacy.add_system_prompt_extra("hints".to_string(), "CALLER_HINTS".to_string());
        legacy.record_tool_arguments(&arguments, project.path());
        let legacy_prompt = legacy
            .builder_with_fresh_hints(project.path(), GooseMode::Auto)
            .build();

        let mut state_machine = PromptManager::with_timestamp(timestamp);
        state_machine.set_system_prompt_override("base".to_string());
        state_machine.add_system_prompt_extra("caller".to_string(), "CALLER_EXTRA".to_string());
        state_machine.add_system_prompt_extra("hints".to_string(), "CALLER_HINTS".to_string());
        let snapshot = reconstructed_hint_snapshot(&conversation, project.path());
        assert!(snapshot.find("ROOT_HINT").unwrap() < snapshot.find("NESTED_HINT").unwrap());
        let state_machine_prompt =
            state_machine.build_system_prompt_from_snapshot(Vec::new(), GooseMode::Auto, snapshot);

        assert_eq!(legacy_prompt, state_machine_prompt);
        assert!(state_machine_prompt.contains("CALLER_EXTRA"));
        assert!(!state_machine_prompt.contains("CALLER_HINTS"));
        assert_eq!(state_machine_prompt.matches("ROOT_HINT").count(), 1);
        assert_eq!(state_machine_prompt.matches("NESTED_HINT").count(), 1);
        let caller_only_prompt = state_machine.builder().build();
        assert!(caller_only_prompt.contains("CALLER_EXTRA"));
        assert!(caller_only_prompt.contains("CALLER_HINTS"));
        assert!(!caller_only_prompt.contains("ROOT_HINT"));
        assert!(!caller_only_prompt.contains("NESTED_HINT"));

        fs::write(project.path().join(GOOSE_HINTS_FILENAME), "UPDATED_ROOT").unwrap();
        assert!(legacy.load_subdirectory_hints(project.path()));
        assert!(legacy.load_subdirectory_hints(project.path()));
        let legacy_refreshed = legacy
            .builder_with_fresh_hints(project.path(), GooseMode::Auto)
            .build();
        let state_machine_refreshed = state_machine.build_system_prompt_from_snapshot(
            Vec::new(),
            GooseMode::Auto,
            reconstructed_hint_snapshot(&conversation, project.path()),
        );
        assert_eq!(legacy_refreshed, state_machine_refreshed);
        assert!(legacy_refreshed.contains("UPDATED_ROOT"));
        assert!(!legacy_refreshed.contains("ROOT_HINT"));
        assert!(legacy_refreshed.contains("NESTED_HINT"));
        assert!(legacy_refreshed.contains("CALLER_EXTRA"));
        assert!(!legacy.load_subdirectory_hints(project.path()));
    }
}
