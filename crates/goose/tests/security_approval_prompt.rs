use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use goose::agents::{
    Agent, AgentConfig, AgentEvent, ExtensionConfig, GoosePlatform, SessionConfig,
};
use goose::config::permission::PermissionManager;
use goose::config::GooseMode;
use goose::conversation::message::{ActionRequiredData, Message, MessageContent};
use goose::permission::permission_confirmation::PrincipalType;
use goose::permission::{Permission, PermissionConfirmation};
use goose::providers::base::{stream_from_single_message, MessageStream, Provider};
use goose::session::{SessionManager, SessionType};
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::object;

const DANGEROUS_COMMAND: &str = concat!(
    "rm -rf / # flagged\nordinary 🪿\n",
    "\u{1b}[2J\u{1b}]0;spoofed\u{7}\u{202e}"
);

struct ApprovalProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for ApprovalProvider {
    fn get_name(&self) -> &str {
        "approval-test"
    }

    async fn stream(
        &self,
        _model_config: &ModelConfig,
        _system: &str,
        _messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let message = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Message::assistant().with_tool_request(
                "dangerous",
                Ok(CallToolRequestParams::new("shell")
                    .with_arguments(object!({"command": DANGEROUS_COMMAND}))),
            )
        } else {
            Message::assistant().with_text("cancelled")
        };
        Ok(stream_from_single_message(
            message,
            ProviderUsage::new("mock-model".to_string(), Usage::default()),
        ))
    }
}

async fn security_approval_prompt(state_machine: Option<&'static str>) -> Result<String> {
    let _guard = env_lock::lock_env([
        ("GOOSE_STATE_MACHINE", state_machine),
        ("SECURITY_PROMPT_ENABLED_OVERRIDE", Some("true")),
        (
            "SECURITY_COMMAND_CLASSIFIER_ENABLED_OVERRIDE",
            Some("false"),
        ),
    ]);
    let data_root = tempfile::tempdir()?;
    let session_manager = Arc::new(SessionManager::new(data_root.path().to_path_buf()));
    let permission_manager = Arc::new(PermissionManager::new(data_root.path().join("permissions")));
    let agent = Agent::with_config(AgentConfig::new(
        session_manager.clone(),
        permission_manager,
        None,
        GooseMode::Auto,
        true,
        GoosePlatform::GooseCli,
    ));
    let session = session_manager
        .create_session(
            PathBuf::default(),
            "security approval".to_string(),
            SessionType::Hidden,
            GooseMode::Auto,
        )
        .await?;
    agent
        .update_provider(
            Arc::new(ApprovalProvider {
                calls: AtomicUsize::new(0),
            }),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;
    agent
        .add_extension(
            ExtensionConfig::Platform {
                name: "developer".to_string(),
                description: "developer".to_string(),
                display_name: None,
                bundled: None,
                available_tools: vec![],
            },
            &session.id,
        )
        .await?;

    let stream = agent
        .reply(
            Message::user().with_text("run a dangerous command"),
            SessionConfig {
                id: session.id,
                schedule_id: None,
                max_turns: Some(3),
                retry_config: None,
            },
            None,
        )
        .await?;
    tokio::pin!(stream);
    let mut approval_prompt = None;
    while let Some(event) = stream.next().await {
        if let AgentEvent::Message(message) = event? {
            for content in message.content {
                let MessageContent::ActionRequired(action) = content else {
                    continue;
                };
                if let ActionRequiredData::ToolConfirmation { id, prompt, .. } = action.data {
                    approval_prompt = prompt;
                    agent
                        .handle_confirmation(
                            id,
                            PermissionConfirmation {
                                principal_type: PrincipalType::Tool,
                                permission: Permission::DenyOnce,
                            },
                        )
                        .await;
                }
            }
        }
    }

    approval_prompt.ok_or_else(|| anyhow::anyhow!("security approval prompt was not emitted"))
}

fn assert_prompt_is_safe(prompt: &str) {
    assert!(prompt.contains("ordinary 🪿\n"));
    assert!(prompt.contains("\\u{1b}[2J"));
    assert!(prompt.contains("\\u{1b}]0;spoofed\\u{7}"));
    assert!(prompt.contains("\\u{202e}"));
    assert!(!prompt.chars().any(|character| {
        character != '\n'
            && (character.is_control()
                || matches!(
                    character,
                    '\u{61c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                ))
    }));
}

#[tokio::test]
async fn legacy_tool_confirmation_uses_safe_security_prompt() -> Result<()> {
    assert_prompt_is_safe(&security_approval_prompt(None).await?);
    Ok(())
}

#[tokio::test]
async fn state_machine_tool_confirmation_uses_safe_security_prompt() -> Result<()> {
    assert_prompt_is_safe(&security_approval_prompt(Some("1")).await?);
    Ok(())
}
