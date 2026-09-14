use crate::agents::{Agent, ExtensionLoadResult};
use crate::config::{Config, GooseMode};
use crate::providers::inventory::{ProviderInventoryEntry, ProviderInventoryService};
use crate::session::session_manager::SessionUsageTotals;
use crate::session::Session;
use crate::slash_commands::types::{SlashCommandEntry, SlashCommandSource};
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionId, SessionInfo, SessionMode,
    SessionModeId, SessionModeState, SessionNotification, SessionUpdate, UnstructuredCommandInput,
};
use agent_client_protocol::{Client, ConnectionTo};
use goose_providers::model::ModelConfig;
use goose_providers::thinking::{ThinkingEffort, ThinkingEffortCapability, ThinkingEffortSupport};
use serde::Serialize;
use strum::{EnumMessage, VariantNames};

use super::provider::resolve_effort_value;
use super::server::{build_usage_updates, DEFAULT_PROVIDER_ID, DEFAULT_PROVIDER_LABEL};

pub(super) fn session_provider_selection(session: &Session) -> &str {
    session
        .provider_name
        .as_deref()
        .unwrap_or(DEFAULT_PROVIDER_ID)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionMeta<'a> {
    message_count: usize,
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    archived_at: Option<chrono::DateTime<chrono::Utc>>,
    user_set_name: bool,
    session_type: String,
    has_recipe: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_message_snippet: Option<&'a str>,
}

impl<'a> From<&'a Session> for SessionMeta<'a> {
    fn from(session: &'a Session) -> Self {
        Self {
            message_count: session.message_count,
            created_at: session.created_at,
            last_message_at: session.last_message_at,
            archived_at: session.archived_at,
            user_set_name: session.user_set_name,
            session_type: session.session_type.to_string(),
            has_recipe: session.recipe.is_some(),
            project_id: session.project_id.as_deref(),
            provider_id: session.provider_name.as_deref(),
            model_id: session
                .model_config
                .as_ref()
                .map(|mc| mc.model_name.as_str()),
            last_message_snippet: session.last_message_snippet.as_deref(),
        }
    }
}

pub(super) fn session_meta(session: &Session) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(SessionMeta::from(session)) {
        Ok(serde_json::Value::Object(meta)) => meta,
        _ => serde_json::Map::new(),
    }
}

pub(super) fn session_response_meta(
    session: &Session,
    extension_results: &[ExtensionLoadResult],
) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    if let Some(recipe) = &session.recipe {
        if let Ok(v) = serde_json::to_value(recipe) {
            meta.insert("recipe".to_string(), v);
        }
    }
    if let Some(values) = &session.user_recipe_values {
        if let Ok(v) = serde_json::to_value(values) {
            meta.insert("userRecipeValues".to_string(), v);
        }
    }
    if let Ok(v) = serde_json::to_value(extension_results) {
        meta.insert("extensionResults".to_string(), v);
    }
    meta.insert(
        "workingDir".to_string(),
        serde_json::Value::String(session.working_dir.to_string_lossy().to_string()),
    );
    meta
}

pub(super) fn build_session_info(session: Session) -> SessionInfo {
    let meta = session_meta(&session);
    let mut info = SessionInfo::new(SessionId::new(session.id), session.working_dir)
        .updated_at(session.updated_at.to_rfc3339())
        .meta(meta);
    if !session.name.is_empty() {
        info = info.title(session.name);
    }
    info
}

/// A model and its label, used to build the "model" session config option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelOption {
    pub id: String,
    pub name: String,
}

/// The currently selected model and the set of available models for a session.
///
/// Replaces the removed `SessionModelState` ACP schema type; goose now surfaces
/// model selection through the generic session config option API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelSelection {
    pub current_model_id: String,
    pub available_models: Vec<ModelOption>,
}

pub(super) fn build_model_state(
    current_model: &str,
    inventory: &ProviderInventoryEntry,
) -> ModelSelection {
    let mut available_models = inventory
        .models
        .iter()
        .map(|model| ModelOption {
            id: model.id.clone(),
            name: model.name.clone(),
        })
        .collect::<Vec<_>>();
    if !available_models
        .iter()
        .any(|model| model.id == current_model)
    {
        available_models.insert(
            0,
            ModelOption {
                id: current_model.to_string(),
                name: current_model.to_string(),
            },
        );
    }
    ModelSelection {
        current_model_id: current_model.to_string(),
        available_models,
    }
}

struct ProviderOptionEntry {
    id: String,
    label: String,
}

async fn list_provider_entries(current_provider: Option<&str>) -> Vec<ProviderOptionEntry> {
    let mut providers = crate::providers::providers()
        .await
        .into_iter()
        .map(|(metadata, _)| ProviderOptionEntry {
            id: metadata.name,
            label: metadata.display_name,
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.id.cmp(&right.id));
    providers.dedup_by(|left, right| left.id == right.id);

    if let Some(current_provider) = current_provider {
        if current_provider != DEFAULT_PROVIDER_ID
            && !providers
                .iter()
                .any(|provider| provider.id == current_provider)
        {
            providers.push(ProviderOptionEntry {
                id: current_provider.to_string(),
                label: current_provider.to_string(),
            });
            providers.sort_by(|left, right| left.id.cmp(&right.id));
        }
    }

    let mut entries = Vec::with_capacity(providers.len() + 1);
    entries.push(ProviderOptionEntry {
        id: DEFAULT_PROVIDER_ID.to_string(),
        label: DEFAULT_PROVIDER_LABEL.to_string(),
    });
    entries.extend(providers);
    entries
}

pub(super) async fn build_provider_options(
    current_provider: Option<&str>,
) -> Vec<SessionConfigSelectOption> {
    list_provider_entries(current_provider)
        .await
        .into_iter()
        .map(|provider| SessionConfigSelectOption::new(provider.id, provider.label))
        .collect()
}

pub(super) fn should_refresh_inventory_for_session_init(entry: &ProviderInventoryEntry) -> bool {
    entry.configured
        && entry.supports_refresh
        && (entry.last_updated_at.is_none() || ProviderInventoryService::is_stale(entry))
}

pub(super) fn build_mode_state(
    current_mode: GooseMode,
) -> Result<SessionModeState, agent_client_protocol::Error> {
    let mut available = Vec::with_capacity(GooseMode::VARIANTS.len());
    for &name in GooseMode::VARIANTS {
        let goose_mode: GooseMode = name.parse().map_err(|_| {
            agent_client_protocol::Error::internal_error() // impossible but satisfy linters
                .data(format!("Failed to parse GooseMode variant: {}", name))
        })?;
        let mut mode = SessionMode::new(SessionModeId::new(name), name);
        mode.description = goose_mode.get_message().map(Into::into);
        available.push(mode);
    }
    Ok(SessionModeState::new(
        SessionModeId::new(current_mode.to_string()),
        available,
    ))
}

/// The provider decides whether goose owns the effort menu; a session without a
/// live provider keeps the model-name based path.
pub(super) async fn agent_thinking_effort_support(agent: &Agent) -> ThinkingEffortSupport {
    match agent.provider().await {
        Ok(provider) => provider.thinking_effort_support(),
        Err(_) => ThinkingEffortSupport::Unspecified,
    }
}

pub(super) async fn build_session_setup_config(
    provider_inventory: &ProviderInventoryService,
    session: &Session,
    effort_support: &ThinkingEffortSupport,
) -> Result<(SessionModeState, Option<Vec<SessionConfigOption>>), agent_client_protocol::Error> {
    let mode_state = build_mode_state(session.goose_mode)?;

    let (Some(provider_name), Some(model_config)) = (
        session.provider_name.as_deref(),
        session.model_config.as_ref(),
    ) else {
        return Ok((mode_state, None));
    };
    let Some(inventory) = provider_inventory
        .find_entry_for_provider(provider_name)
        .await
    else {
        return Ok((mode_state, None));
    };
    let model_state = build_model_state(model_config.model_name.as_str(), &inventory);
    let provider_selection = session_provider_selection(session);
    let provider_options = build_provider_options(Some(provider_name)).await;
    let config_options = build_config_options(
        &mode_state,
        &model_state,
        model_config,
        provider_selection,
        provider_options,
        effort_support,
    );
    Ok((mode_state, Some(config_options)))
}

pub(super) fn build_config_options(
    mode_state: &SessionModeState,
    model_state: &ModelSelection,
    model_config: &ModelConfig,
    provider_selection: &str,
    provider_options: Vec<SessionConfigSelectOption>,
    effort_support: &ThinkingEffortSupport,
) -> Vec<SessionConfigOption> {
    let mode_options: Vec<SessionConfigSelectOption> = mode_state
        .available_modes
        .iter()
        .map(|m| {
            SessionConfigSelectOption::new(m.id.0.clone(), m.name.clone())
                .description(m.description.clone())
        })
        .collect();
    let model_options: Vec<SessionConfigSelectOption> = model_state
        .available_models
        .iter()
        .map(|m| SessionConfigSelectOption::new(m.id.clone(), m.name.clone()))
        .collect();
    let (thinking_effort_options, current_thinking_effort) =
        build_thinking_effort_choices(model_config, effort_support);
    vec![
        SessionConfigOption::select(
            "provider",
            "Provider",
            provider_selection.to_string(),
            provider_options,
        ),
        SessionConfigOption::select(
            "mode",
            "Mode",
            mode_state.current_mode_id.0.clone(),
            mode_options,
        )
        .category(SessionConfigOptionCategory::Mode),
        SessionConfigOption::select(
            "model",
            "Model",
            model_state.current_model_id.clone(),
            model_options,
        )
        .category(SessionConfigOptionCategory::Model),
        SessionConfigOption::select(
            "thinking_effort",
            "Thinking effort",
            current_thinking_effort,
            thinking_effort_options,
        )
        .description("Controls reasoning effort for models that support extended thinking.")
        .category(SessionConfigOptionCategory::ThoughtLevel),
    ]
}

/// A provider that manages its own reasoning decides the menu: it mirrors the
/// agent's effort selector verbatim, or honestly offers nothing when the agent
/// has no such knob for the current model. Only providers that leave effort to
/// goose fall back to the model-name based values, which the ACP sentinel model
/// name can never satisfy.
fn build_thinking_effort_choices(
    model_config: &ModelConfig,
    effort_support: &ThinkingEffortSupport,
) -> (Vec<SessionConfigSelectOption>, String) {
    match effort_support {
        ThinkingEffortSupport::Options(capability) => (
            capability
                .values
                .iter()
                .map(|option| {
                    SessionConfigSelectOption::new(option.value.clone(), option.label.clone())
                })
                .collect(),
            capability_thinking_effort_value(capability, model_config),
        ),
        ThinkingEffortSupport::Unsupported => {
            let off = ThinkingEffort::Off.to_string();
            (
                vec![SessionConfigSelectOption::new(off.clone(), off.clone())],
                off,
            )
        }
        ThinkingEffortSupport::Unspecified => (
            thinking_effort_values(model_config)
                .iter()
                .map(|effort| {
                    let effort = effort.to_string();
                    SessionConfigSelectOption::new(effort.clone(), effort)
                })
                .collect(),
            current_thinking_effort_value(model_config),
        ),
    }
}

/// The goose-side value that will actually be sent wins — `resolve_effort_value`
/// is shared with the provider's send path — then whatever the agent currently
/// has, which is what it keeps when goose sends nothing.
fn capability_thinking_effort_value(
    capability: &ThinkingEffortCapability,
    model_config: &ModelConfig,
) -> String {
    resolve_effort_value(capability, model_config)
        .or_else(|| capability.current.clone())
        .or_else(|| capability.values.first().map(|option| option.value.clone()))
        .unwrap_or_default()
}

fn thinking_effort_values(model_config: &ModelConfig) -> &'static [ThinkingEffort] {
    if model_config.is_reasoning_model() {
        &[
            ThinkingEffort::Off,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
            ThinkingEffort::Max,
        ]
    } else {
        &[ThinkingEffort::Off]
    }
}

fn current_thinking_effort_value(model_config: &ModelConfig) -> String {
    if model_config.is_reasoning_model() {
        model_config
            .thinking_effort()
            .or_else(|| Config::global().get_goose_thinking_effort())
            .map(|effort| effort.to_string())
            .unwrap_or_else(|| "off".to_string())
    } else {
        "off".to_string()
    }
}

fn slash_command_meta(entry: &SlashCommandEntry) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    let command_type = match entry.source {
        SlashCommandSource::Builtin => "Builtin",
        SlashCommandSource::Recipe => "Recipe",
        SlashCommandSource::Skill => "Skill",
    };
    meta.insert(
        "commandType".to_string(),
        serde_json::Value::String(command_type.to_string()),
    );
    if let Some(source_path) = &entry.source_path {
        meta.insert(
            "sourcePath".to_string(),
            serde_json::Value::String(source_path.clone()),
        );
    }
    meta
}

fn slash_command_to_available_command(entry: SlashCommandEntry) -> AvailableCommand {
    let meta = slash_command_meta(&entry);
    let mut command = AvailableCommand::new(entry.name, entry.description);
    if let Some(input_hint) = entry.input_hint {
        command = command.input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new(input_hint),
        ));
    }
    command.meta(meta)
}

pub(super) fn available_commands_for_working_dir(
    working_dir: &std::path::Path,
) -> Vec<AvailableCommand> {
    available_commands_for_optional_working_dir(Some(working_dir))
}

pub(super) fn available_commands_for_optional_working_dir(
    working_dir: Option<&std::path::Path>,
) -> Vec<AvailableCommand> {
    crate::slash_commands::slash_command::list_acp_commands(working_dir)
        .into_iter()
        .map(slash_command_to_available_command)
        .collect()
}

fn available_commands_update(working_dir: &std::path::Path) -> AvailableCommandsUpdate {
    AvailableCommandsUpdate::new(available_commands_for_working_dir(working_dir))
}

pub(super) fn send_session_setup_notifications(
    cx: &ConnectionTo<Client>,
    session: &Session,
    totals: &SessionUsageTotals,
    context_limit: usize,
    supports_goose_custom_notifications: bool,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = SessionId::new(session.id.clone());
    let updates = build_usage_updates(session, totals, context_limit);
    if supports_goose_custom_notifications {
        cx.send_notification(updates.custom)?;
    }
    cx.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::UsageUpdate(updates.standard),
    ))?;
    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AvailableCommandsUpdate(available_commands_update(&session.working_dir)),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::provider::THINKING_EFFORT_PARAM;
    use super::*;
    use agent_client_protocol::schema::v1::SessionConfigKind;
    use goose_providers::thinking::ThinkingEffortOption;
    use test_case::test_case;

    fn model_selection(current: &str, models: &[&str]) -> ModelSelection {
        ModelSelection {
            current_model_id: current.to_string(),
            available_models: models
                .iter()
                .map(|m| ModelOption {
                    id: m.to_string(),
                    name: m.to_string(),
                })
                .collect(),
        }
    }

    #[test_case(
        vec!["model-a".into(), "model-b".into()]
        => model_selection("unused", &["unused", "model-a", "model-b"])
        ; "returns current and available models"
    )]
    #[test_case(
        vec![]
        => model_selection("unused", &["unused"])
        ; "empty model list"
    )]
    fn test_build_model_state(models: Vec<String>) -> ModelSelection {
        let inventory = ProviderInventoryEntry {
            provider_id: "mock".to_string(),
            provider_name: "Mock".to_string(),
            description: "Mock".to_string(),
            default_model: "unused".to_string(),
            configured: true,
            available: true,
            provider_type: crate::providers::base::ProviderType::Builtin,
            acp: false,
            visible_in_setup: true,
            deprecated: false,
            replacement: None,
            config_keys: vec![],
            setup_steps: vec![],
            supports_refresh: true,
            refreshing: false,
            models: models
                .into_iter()
                .map(|id| crate::providers::inventory::InventoryModel {
                    name: id.clone(),
                    id,
                    family: None,
                    context_limit: None,
                    reasoning: None,
                    recommended: false,
                })
                .collect(),
            last_updated_at: None,
            last_refresh_attempt_at: None,
            last_refresh_error: None,
        };
        build_model_state("unused", &inventory)
    }

    #[test_case(
        GooseMode::Auto
        => Ok(SessionModeState::new(
            SessionModeId::new("auto"),
            vec![
                SessionMode::new(SessionModeId::new("auto"), "auto")
                    .description("Automatically approve tool calls"),
                SessionMode::new(SessionModeId::new("approve"), "approve")
                    .description("Ask before every tool call"),
                SessionMode::new(SessionModeId::new("smart_approve"), "smart_approve")
                    .description("Ask only for sensitive tool calls"),
                SessionMode::new(SessionModeId::new("chat"), "chat")
                    .description("Chat only, no tool calls"),
            ],
        ))
        ; "auto mode"
    )]
    #[test_case(
        GooseMode::Approve
        => Ok(SessionModeState::new(
            SessionModeId::new("approve"),
            vec![
                SessionMode::new(SessionModeId::new("auto"), "auto")
                    .description("Automatically approve tool calls"),
                SessionMode::new(SessionModeId::new("approve"), "approve")
                    .description("Ask before every tool call"),
                SessionMode::new(SessionModeId::new("smart_approve"), "smart_approve")
                    .description("Ask only for sensitive tool calls"),
                SessionMode::new(SessionModeId::new("chat"), "chat")
                    .description("Chat only, no tool calls"),
            ],
        ))
        ; "approve mode"
    )]
    fn test_build_mode_state(
        current_mode: GooseMode,
    ) -> Result<SessionModeState, agent_client_protocol::Error> {
        build_mode_state(current_mode)
    }

    #[test]
    fn test_slash_command_to_available_command_maps_core_fields_to_acp() {
        let cases = [
            (SlashCommandSource::Builtin, "Builtin", None),
            (
                SlashCommandSource::Recipe,
                "Recipe",
                Some("/tmp/release.yaml".to_string()),
            ),
            (SlashCommandSource::Skill, "Skill", None),
        ];

        for (source, expected_command_type, expected_source_path) in cases {
            let command = slash_command_to_available_command(SlashCommandEntry {
                name: "release".to_string(),
                description: "Run release workflow".to_string(),
                source,
                source_path: expected_source_path.clone(),
                input_hint: Some("[task]".to_string()),
            });

            assert_eq!(command.name, "release");
            assert_eq!(command.description, "Run release workflow");

            match command.input.as_ref() {
                Some(AvailableCommandInput::Unstructured(input)) => {
                    assert_eq!(input.hint, "[task]");
                }
                other => panic!("unexpected command input: {other:?}"),
            }

            let meta = command.meta.as_ref().expect("command _meta");
            let expected_command_type = serde_json::json!(expected_command_type);
            assert_eq!(meta.get("commandType"), Some(&expected_command_type));
            if let Some(source_path) = expected_source_path {
                let expected_source_path = serde_json::json!(source_path);
                assert_eq!(meta.get("sourcePath"), Some(&expected_source_path));
            } else {
                assert!(meta.get("sourcePath").is_none());
            }
        }
    }

    #[test_case(
        build_mode_state(GooseMode::Auto).unwrap(),
        "openai",
        vec![
            SessionConfigSelectOption::new("anthropic", "anthropic"),
            SessionConfigSelectOption::new("openai", "openai"),
        ],
        model_selection("gpt-4", &["gpt-4", "gpt-3.5"])
        => vec![
            SessionConfigOption::select(
                "provider", "Provider", "openai",
                vec![
                    SessionConfigSelectOption::new("anthropic", "anthropic"),
                    SessionConfigSelectOption::new("openai", "openai"),
                ],
            ),
            SessionConfigOption::select(
                "mode", "Mode", "auto",
                vec![
                    SessionConfigSelectOption::new("auto", "auto").description("Automatically approve tool calls"),
                    SessionConfigSelectOption::new("approve", "approve").description("Ask before every tool call"),
                    SessionConfigSelectOption::new("smart_approve", "smart_approve").description("Ask only for sensitive tool calls"),
                    SessionConfigSelectOption::new("chat", "chat").description("Chat only, no tool calls"),
                ],
            ).category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select(
                "model", "Model", "gpt-4",
                vec![
                    SessionConfigSelectOption::new("gpt-4", "gpt-4"),
                    SessionConfigSelectOption::new("gpt-3.5", "gpt-3.5"),
                ],
            ).category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "thinking_effort", "Thinking effort", "off",
                vec![SessionConfigSelectOption::new("off", "off")],
            )
            .description("Controls reasoning effort for models that support extended thinking.")
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ]
        ; "auto mode with multiple models"
    )]
    #[test_case(
        build_mode_state(GooseMode::Approve).unwrap(),
        "openai",
        vec![SessionConfigSelectOption::new("openai", "openai")],
        model_selection("only-model", &["only-model"])
        => vec![
            SessionConfigOption::select(
                "provider", "Provider", "openai",
                vec![SessionConfigSelectOption::new("openai", "openai")],
            ),
            SessionConfigOption::select(
                "mode", "Mode", "approve",
                vec![
                    SessionConfigSelectOption::new("auto", "auto").description("Automatically approve tool calls"),
                    SessionConfigSelectOption::new("approve", "approve").description("Ask before every tool call"),
                    SessionConfigSelectOption::new("smart_approve", "smart_approve").description("Ask only for sensitive tool calls"),
                    SessionConfigSelectOption::new("chat", "chat").description("Chat only, no tool calls"),
                ],
            ).category(SessionConfigOptionCategory::Mode),
            SessionConfigOption::select(
                "model", "Model", "only-model",
                vec![SessionConfigSelectOption::new("only-model", "only-model")],
            ).category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "thinking_effort", "Thinking effort", "off",
                vec![SessionConfigSelectOption::new("off", "off")],
            )
            .description("Controls reasoning effort for models that support extended thinking.")
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ]
        ; "approve mode with single model"
    )]
    fn test_build_config_options(
        mode_state: SessionModeState,
        provider_name: &'static str,
        provider_options: Vec<SessionConfigSelectOption>,
        model_state: ModelSelection,
    ) -> Vec<SessionConfigOption> {
        let model_config = ModelConfig::new(model_state.current_model_id.as_str())
            .with_merged_request_params(std::collections::HashMap::from([(
                "thinking_effort".to_string(),
                serde_json::json!("off"),
            )]));
        build_config_options(
            &mode_state,
            &model_state,
            &model_config,
            provider_name,
            provider_options,
            &ThinkingEffortSupport::Unspecified,
        )
    }

    #[test]
    fn test_build_config_options_uses_current_thinking_effort() {
        let mode_state = build_mode_state(GooseMode::Auto).unwrap();
        let model_state = model_selection("claude-sonnet-4", &["claude-sonnet-4"]);
        let model_config = ModelConfig::new("claude-sonnet-4").with_merged_request_params(
            std::collections::HashMap::from([(
                "thinking_effort".to_string(),
                serde_json::json!("high"),
            )]),
        );

        let options = build_config_options(
            &mode_state,
            &model_state,
            &model_config,
            "openai",
            vec![SessionConfigSelectOption::new("openai", "openai")],
            &ThinkingEffortSupport::Unspecified,
        );
        let option = options
            .iter()
            .find(|option| option.id.0.as_ref() == "thinking_effort")
            .expect("thinking_effort option");
        let select = match &option.kind {
            SessionConfigKind::Select(select) => select,
            _ => panic!("thinking_effort should be a select option"),
        };

        assert_eq!(select.current_value.0.as_ref(), "high");
    }

    #[test]
    fn test_build_config_options_masks_non_reasoning_thinking_effort() {
        let mode_state = build_mode_state(GooseMode::Auto).unwrap();
        let model_state = model_selection("gpt-4", &["gpt-4"]);
        let mut model_config =
            ModelConfig::new("gpt-4").with_merged_request_params(std::collections::HashMap::from(
                [("thinking_effort".to_string(), serde_json::json!("high"))],
            ));
        model_config.reasoning = Some(false);

        let options = build_config_options(
            &mode_state,
            &model_state,
            &model_config,
            "openai",
            vec![SessionConfigSelectOption::new("openai", "openai")],
            &ThinkingEffortSupport::Unspecified,
        );
        let option = options
            .iter()
            .find(|option| option.id.0.as_ref() == "thinking_effort")
            .expect("thinking_effort option");
        let select = match &option.kind {
            SessionConfigKind::Select(select) => select,
            _ => panic!("thinking_effort should be a select option"),
        };

        assert_eq!(select.current_value.0.as_ref(), "off");
        assert_eq!(
            select.options,
            agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(vec![
                SessionConfigSelectOption::new("off", "off")
            ])
        );
    }

    fn effort_capability(values: &[&str], current: &str) -> ThinkingEffortCapability {
        ThinkingEffortCapability {
            option_id: "effort".to_string(),
            values: values
                .iter()
                .map(|value| ThinkingEffortOption {
                    value: value.to_string(),
                    label: value.to_string(),
                })
                .collect(),
            current: Some(current.to_string()),
        }
    }

    fn build_effort_option(
        model_name: &str,
        persisted_effort: Option<&str>,
        effort_support: &ThinkingEffortSupport,
    ) -> (String, Vec<SessionConfigSelectOption>) {
        let mode_state = build_mode_state(GooseMode::Auto).unwrap();
        let model_state = model_selection(model_name, &[model_name]);
        let mut model_config = ModelConfig::new(model_name);
        if let Some(effort) = persisted_effort {
            model_config =
                model_config.with_merged_request_params(std::collections::HashMap::from([(
                    THINKING_EFFORT_PARAM.to_string(),
                    serde_json::json!(effort),
                )]));
        }

        let options = build_config_options(
            &mode_state,
            &model_state,
            &model_config,
            "claude-acp",
            vec![SessionConfigSelectOption::new("claude-acp", "claude-acp")],
            effort_support,
        );
        let option = options
            .into_iter()
            .find(|option| option.id.0.as_ref() == "thinking_effort")
            .expect("thinking_effort option");
        let SessionConfigKind::Select(select) = option.kind else {
            panic!("thinking_effort should be a select option");
        };
        let values = match select.options {
            agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(values) => {
                values
            }
            grouped => panic!("unexpected thinking_effort options: {grouped:?}"),
        };
        (select.current_value.0.to_string(), values)
    }

    #[test]
    fn test_build_config_options_mirrors_agent_effort_menu_on_the_model_sentinel() {
        let model_name = crate::acp::ACP_CURRENT_MODEL;
        assert!(
            !ModelConfig::new(model_name).is_reasoning_model(),
            "the sentinel is the model name that made the old sniffing path collapse the menu"
        );
        let support = ThinkingEffortSupport::Options(effort_capability(
            &["default", "high", "xhigh"],
            "high",
        ));

        let (current, values) = build_effort_option(model_name, Some("high"), &support);

        assert_eq!(current, "high");
        assert_eq!(
            values,
            vec![
                SessionConfigSelectOption::new("default", "default"),
                SessionConfigSelectOption::new("high", "high"),
                SessionConfigSelectOption::new("xhigh", "xhigh"),
            ]
        );
    }

    #[test_case(Some("high") => "high".to_string() ; "persisted value")]
    #[test_case(Some("off") => "default".to_string() ; "persisted off maps onto the agent default")]
    #[test_case(Some("max") => "xhigh".to_string() ; "persisted max maps onto the agent's xhigh")]
    #[test_case(Some("unknown") => "low".to_string() ; "unmappable persisted value falls back to the agent")]
    #[test_case(None => "low".to_string() ; "no persisted value falls back to the agent")]
    fn test_build_config_options_effort_current_value(persisted: Option<&str>) -> String {
        // Unoffered by the capability below, so the global default never wins
        // and the test doesn't depend on the machine's configured value.
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("medium"))]);
        let support = ThinkingEffortSupport::Options(effort_capability(
            &["default", "low", "high", "xhigh"],
            "low",
        ));

        build_effort_option(crate::acp::ACP_CURRENT_MODEL, persisted, &support).0
    }

    #[test]
    fn test_build_config_options_effort_falls_back_to_the_global_default() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("high"))]);
        let support =
            ThinkingEffortSupport::Options(effort_capability(&["default", "high"], "default"));

        let (current, _) = build_effort_option(crate::acp::ACP_CURRENT_MODEL, None, &support);

        assert_eq!(current, "high");
    }

    #[test]
    fn test_build_config_options_offers_no_effort_when_the_agent_has_none() {
        let (current, values) = build_effort_option(
            "claude-sonnet-4",
            Some("high"),
            &ThinkingEffortSupport::Unsupported,
        );

        assert_eq!(current, "off");
        assert_eq!(values, vec![SessionConfigSelectOption::new("off", "off")]);
    }
}
