mod builder;
mod completion;
pub mod editor;
mod elicitation;
mod input;
mod output;
mod paste;
pub mod streaming_buffer;
mod task_execution_display;
mod thinking;

use crate::session::task_execution_display::{
    format_task_execution_notification, TASK_EXECUTION_NOTIFICATION_TYPE,
};
use goose::conversation::{fix_conversation, merge_consecutive_messages_for_request, Conversation};
use std::io::Write;
use std::str::FromStr;
use tokio::signal::ctrl_c;
use tokio_util::task::AbortOnDropHandle;

pub use builder::{build_session, ExtensionFailure, SessionBuilderConfig};
use console::Color;

use goose::agents::platform_extensions::developer::shell::{
    parse_shell_output_notification, ShellOutputNotificationParams, ShellOutputStream,
};
use goose::agents::AgentEvent;
use goose::agents::SUBAGENT_TOOL_REQUEST_TYPE;
use goose::permission::Permission;
use goose::providers::base::Provider;
use goose::providers::base::ProviderUsage;
use goose::utils::safe_truncate;

use anyhow::Result;
use completion::GooseCompleter;
use goose::agents::extension::{Envs, ExtensionConfig, PLATFORM_EXTENSIONS};
use goose::agents::types::RetryConfig;
use goose::agents::{
    context_management_unsupported_message, Agent, SessionConfig, COMPACT_TRIGGERS,
};
use goose::config::extensions::name_to_key;
use goose::config::{Config, GooseMode};
use input::InputResult;
use rmcp::model::ServerNotification;
use rmcp::model::{ElicitationAction, PromptMessage};
use rmcp::model::{ErrorCode, ErrorData};
use strum::VariantNames;

use goose::config::paths::Paths;
use goose::config::providers;
use goose::conversation::message::{
    ActionRequiredData, Message, MessageContent, ToolConfirmationRequest,
};
use goose::providers::inventory::ProviderInventoryService;
use goose::session::SessionManager;
use rustyline::EditMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio;
use tokio_util::sync::CancellationToken;
use tracing::warn;

const SHELL_STATUS_FALLBACK_WIDTH: usize = 120;
const SHELL_STATUS_MAX_LINES: usize = 3;
const SHELL_STATUS_RESERVED_WIDTH: usize = 2;

/// Upper bound on a *derived* extension name. Tools are exposed to the model as
/// `{extension}__{tool}`, and several providers cap tool name length, so a name
/// built out of a whole command line has to be clipped.
const DERIVED_EXTENSION_NAME_MAX_LEN: usize = 32;

pub(crate) fn split_extension_name_prefix(extension_command: &str) -> (Option<String>, &str) {
    let Some((candidate, rest)) = extension_command.split_once(':') else {
        return (None, extension_command);
    };
    let looks_like_name = candidate.chars().count() >= 2
        && candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !looks_like_name || rest.trim().is_empty() || rest.starts_with("//") {
        return (None, extension_command);
    }
    (Some(name_to_key(candidate)), rest)
}

pub(crate) fn derive_extension_name_from_command(cmd: &str, args: &[String]) -> String {
    let basename = std::path::Path::new(cmd)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(cmd);
    let joined = std::iter::once(basename)
        .chain(args.iter().map(String::as_str))
        .map(|token| token.trim_start_matches('-'))
        .collect::<Vec<_>>()
        .join("_");

    let mut key = String::with_capacity(joined.len());
    for c in name_to_key(&joined).chars() {
        if c == '_' && key.ends_with('_') {
            continue;
        }
        key.push(c);
    }
    let key = key.trim_matches('_');

    if key.chars().count() <= DERIVED_EXTENSION_NAME_MAX_LEN {
        return key.to_string();
    }
    let tail: String = key
        .chars()
        .skip(key.chars().count() - DERIVED_EXTENSION_NAME_MAX_LEN)
        .collect();
    // Drop the leading fragment of whatever token the clip landed inside.
    match tail.split_once('_') {
        Some((_, rest)) if !rest.is_empty() => rest.to_string(),
        _ => tail,
    }
}

fn planner_provider_messages(plan_messages: &Conversation) -> Conversation {
    // The planner prompt has no turn-context instructions; drop the blocks.
    let projected_messages: Vec<Message> = plan_messages
        .agent_visible_messages()
        .into_iter()
        .filter(|message| !message.is_turn_context())
        .collect();
    let fixed = fix_conversation(Conversation::new_unvalidated(projected_messages)).0;
    Conversation::new_unvalidated(merge_consecutive_messages_for_request(
        fixed.messages().clone(),
    ))
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonOutput {
    messages: Vec<Message>,
    metadata: JsonMetadata,
}

#[derive(Serialize, Deserialize, Debug)]
struct JsonMetadata {
    total_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_write_input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    status: String,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    Message {
        message: Message,
    },
    Notification {
        extension_id: String,
        #[serde(flatten)]
        data: NotificationData,
    },
    Error {
        error: String,
    },
    Complete {
        total_tokens: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_tokens: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input_tokens: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_write_input_tokens: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "snake_case")]
enum NotificationData {
    Log {
        message: String,
    },
    Progress {
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    },
}

pub enum RunMode {
    Normal,
    Plan,
}

struct HistoryManager {
    history_file: PathBuf,
    old_history_file: PathBuf,
}

impl HistoryManager {
    fn new() -> Self {
        Self {
            history_file: Paths::state_dir().join("history.txt"),
            old_history_file: Paths::config_dir().join("history.txt"),
        }
    }

    fn load(
        &self,
        editor: &mut rustyline::Editor<GooseCompleter, rustyline::history::DefaultHistory>,
    ) {
        if let Some(parent) = self.history_file.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("Warning: Failed to create history directory: {}", e);
                }
            }
        }

        let history_files = [&self.history_file, &self.old_history_file];
        if let Some(file) = history_files.iter().find(|f| f.exists()) {
            if let Err(err) = editor.load_history(file) {
                eprintln!("Warning: Failed to load command history: {}", err);
            }
        }
    }

    fn save(
        &self,
        editor: &mut rustyline::Editor<GooseCompleter, rustyline::history::DefaultHistory>,
    ) {
        if let Err(err) = editor.save_history(&self.history_file) {
            eprintln!("Warning: Failed to save command history: {}", err);
        } else if self.old_history_file.exists() {
            if let Err(err) = std::fs::remove_file(&self.old_history_file) {
                eprintln!("Warning: Failed to remove old history file: {}", err);
            }
        }
    }
}

pub struct CliSession {
    agent: Arc<Agent>,
    messages: Conversation,
    session_id: String,
    completion_cache: Arc<std::sync::RwLock<CompletionCache>>,
    debug: bool,
    run_mode: RunMode,
    scheduled_job_id: Option<String>,
    max_turns: Option<u32>,
    edit_mode: Option<EditMode>,
    retry_config: Option<RetryConfig>,
    output_format: String,
    stats: bool,
    /// Background extension loader; drained exclusively by
    /// [`CliSession::ensure_extensions_loaded`], the session's single loading
    /// gate.
    extension_loading: Option<AbortOnDropHandle<Result<Vec<ExtensionFailure>>>>,
    loading_announced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintStatus {
    Default,
    Interrupted,
    MaybeExit,
}

// Cache structure for completion data
pub struct CompletionCache {
    pub prompts: HashMap<String, Vec<String>>,
    pub prompt_info: HashMap<String, output::PromptInfo>,
    pub provider_names: Vec<String>,
    pub provider_models: HashMap<String, Vec<String>>,
    pub current_session_provider: String,
    pub last_updated: Instant,
    pub hint_status: HintStatus,
}

impl CompletionCache {
    fn new() -> Self {
        Self {
            prompts: HashMap::new(),
            prompt_info: HashMap::new(),
            provider_names: Vec::new(),
            provider_models: HashMap::new(),
            current_session_provider: String::new(),
            last_updated: Instant::now(),
            hint_status: HintStatus::Default,
        }
    }
}

pub enum PlannerResponseType {
    Plan,
    ClarifyingQuestions,
}

/// Decide if the planner's response is a plan or a clarifying question
///
/// This function is called after the planner has generated a response
/// to the user's message. The response is either a plan or a clarifying
/// question.
pub async fn classify_planner_response(
    session_id: &str,
    message_text: String,
    provider: Arc<dyn Provider>,
    model_config: goose_providers::model::ModelConfig,
) -> Result<PlannerResponseType> {
    let prompt = format!(
        "The text below is the output from an AI model which can either provide a plan or list of clarifying questions. Based on the text below, decide if the output is a \"plan\" or \"clarifying questions\".\n---\n{message_text}"
    );

    let message = Message::user().with_text(&prompt);
    let (result, _usage) = goose::session_context::with_session_id(
        Some(session_id.to_string()),
        provider.complete(
            &model_config,
            "Reply only with the classification label: \"plan\" or \"clarifying questions\"",
            &[message],
            &[],
        ),
    )
    .await?;

    let predicted = result.as_concat_text();
    if predicted.to_lowercase().contains("plan") {
        Ok(PlannerResponseType::Plan)
    } else {
        Ok(PlannerResponseType::ClarifyingQuestions)
    }
}

fn planner_classification_text(response: &Message) -> Result<String> {
    let text = response.agent_visible_content().as_concat_text();
    anyhow::ensure!(
        !text.trim().is_empty(),
        "Planner returned no agent-visible text to classify"
    );
    Ok(text)
}

impl CliSession {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        agent: Arc<Agent>,
        session_id: String,
        debug: bool,
        scheduled_job_id: Option<String>,
        max_turns: Option<u32>,
        edit_mode: Option<EditMode>,
        retry_config: Option<RetryConfig>,
        output_format: String,
        stats: bool,
        refresh_completions: bool,
        extension_loading: Option<AbortOnDropHandle<Vec<ExtensionFailure>>>,
    ) -> Self {
        let messages = agent
            .config
            .session_manager
            .get_session(&session_id, true)
            .await
            .map(|session| session.conversation.unwrap_or_default())
            .unwrap();
        let completion_cache = Arc::new(std::sync::RwLock::new(CompletionCache::new()));
        let extension_loading = extension_loading.map(|handle| {
            let agent = agent.clone();
            let session_id = session_id.clone();
            let completion_cache = completion_cache.clone();
            AbortOnDropHandle::new(tokio::spawn(async move {
                let failures = handle
                    .await
                    .map_err(|error| anyhow::anyhow!("Extension loading task failed: {}", error))?;
                if refresh_completions {
                    Self::refresh_completion_cache(&agent, &session_id, &completion_cache).await?;
                }
                Ok(failures)
            }))
        });

        CliSession {
            agent,
            messages,
            session_id,
            completion_cache,
            debug,
            run_mode: RunMode::Normal,
            scheduled_job_id,
            max_turns,
            edit_mode,
            retry_config,
            output_format,
            stats,
            extension_loading,
            loading_announced: false,
        }
    }

    pub fn session_id(&self) -> &String {
        &self.session_id
    }

    /// Parse a stdio extension command string into an ExtensionConfig
    /// Format: "[name:]ENV1=val1 ENV2=val2 command args..."
    ///
    /// Without the optional `name:` prefix the extension is named after the
    /// basename of the command, which is the launcher rather than the server
    /// whenever one is used (`npx`, `python -m ...`, `uvx`, ...).
    pub fn parse_stdio_extension(extension_command: &str) -> Result<ExtensionConfig> {
        let (explicit_name, command) = split_extension_name_prefix(extension_command);
        let mut parts = goose::utils::split_command_args(command)?;
        let mut envs = HashMap::new();

        while let Some(part) = parts.first() {
            if !part.contains('=') {
                break;
            }
            let env_part = parts.remove(0);
            let (key, value) = env_part.split_once('=').unwrap();
            envs.insert(key.to_string(), value.to_string());
        }

        if parts.is_empty() {
            return Err(anyhow::anyhow!("No command provided in extension string"));
        }

        let cmd = parts.remove(0);
        let name = explicit_name.unwrap_or_else(|| {
            std::path::Path::new(&cmd)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("unnamed")
                .to_string()
        });

        Ok(ExtensionConfig::Stdio {
            name,
            cmd,
            args: parts,
            envs: Envs::new(envs),
            env_keys: Vec::new(),
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(goose::config::DEFAULT_EXTENSION_TIMEOUT),
            cwd: None,
            bundled: None,
            available_tools: Vec::new(),
        })
    }

    pub fn parse_streamable_http_extension(extension_url: &str, timeout: u64) -> ExtensionConfig {
        let name = url::Url::parse(extension_url)
            .ok()
            .map(|u| {
                let mut s = String::new();
                if let Some(host) = u.host_str() {
                    s.push_str(host);
                }
                if let Some(port) = u.port() {
                    s.push('_');
                    s.push_str(&port.to_string());
                }
                let path = u.path().trim_matches('/');
                if !path.is_empty() {
                    s.push('_');
                    s.push_str(path);
                }
                name_to_key(&s)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unnamed".to_string());

        ExtensionConfig::StreamableHttp {
            name,
            uri: extension_url.to_string(),
            envs: Envs::new(HashMap::new()),
            env_keys: Vec::new(),
            headers: HashMap::new(),
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(timeout),
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: Vec::new(),
        }
    }

    /// Parse builtin extension names (comma-separated) into ExtensionConfigs
    pub fn parse_builtin_extensions(builtin_name: &str) -> Vec<ExtensionConfig> {
        builtin_name
            .split(',')
            .map(|name| {
                let extension_name = name.trim();
                if PLATFORM_EXTENSIONS.contains_key(extension_name) {
                    ExtensionConfig::Platform {
                        name: extension_name.to_string(),
                        description: extension_name.to_string(),
                        display_name: None,
                        bundled: None,
                        available_tools: Vec::new(),
                    }
                } else {
                    ExtensionConfig::Builtin {
                        name: extension_name.to_string(),
                        display_name: None,
                        timeout: None,
                        bundled: None,
                        description: extension_name.to_string(),
                        available_tools: Vec::new(),
                    }
                }
            })
            .collect()
    }

    async fn add_and_persist_extensions(&mut self, configs: Vec<ExtensionConfig>) -> Result<()> {
        // Extension-set mutations must not race the background loader.
        self.ensure_extensions_loaded(true).await?;
        for config in configs {
            self.agent
                .add_extension(config, &self.session_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to start extension: {}", e))?;
        }

        self.invalidate_completion_cache().await;

        Ok(())
    }

    pub async fn add_extension(&mut self, extension_command: String) -> Result<()> {
        let config = Self::parse_stdio_extension(&extension_command)?;
        self.add_and_persist_extensions(vec![config]).await
    }

    pub async fn add_streamable_http_extension(&mut self, extension_url: String) -> Result<()> {
        let config = Self::parse_streamable_http_extension(
            &extension_url,
            goose::config::DEFAULT_EXTENSION_TIMEOUT,
        );
        self.add_and_persist_extensions(vec![config]).await
    }

    pub async fn add_builtin(&mut self, builtin_name: String) -> Result<()> {
        let configs = Self::parse_builtin_extensions(&builtin_name);
        self.add_and_persist_extensions(configs).await
    }

    pub async fn list_prompts(
        &mut self,
        extension: Option<String>,
    ) -> Result<HashMap<String, Vec<String>>> {
        self.ensure_extensions_loaded(true).await?;
        let prompts = self.agent.list_extension_prompts(&self.session_id).await;

        // Early validation if filtering by extension
        if let Some(filter) = &extension {
            if !prompts.contains_key(filter) {
                return Err(anyhow::anyhow!("Extension '{}' not found", filter));
            }
        }

        // Convert prompts into filtered map of extension names to prompt names
        Ok(prompts
            .into_iter()
            .filter(|(ext, _)| extension.as_ref().is_none_or(|f| f == ext))
            .map(|(extension, prompt_list)| {
                let names = prompt_list.into_iter().map(|p| p.name).collect();
                (extension, names)
            })
            .collect())
    }

    pub async fn get_prompt_info(&mut self, name: &str) -> Result<Option<output::PromptInfo>> {
        self.ensure_extensions_loaded(true).await?;
        let prompts = self.agent.list_extension_prompts(&self.session_id).await;

        // Find which extension has this prompt
        for (extension, prompt_list) in prompts {
            if let Some(prompt) = prompt_list.iter().find(|p| p.name == name) {
                return Ok(Some(output::PromptInfo {
                    name: prompt.name.clone(),
                    description: prompt.description.clone(),
                    arguments: prompt.arguments.clone(),
                    extension: Some(extension),
                }));
            }
        }

        Ok(None)
    }

    pub async fn get_prompt(&mut self, name: &str, arguments: Value) -> Result<Vec<PromptMessage>> {
        self.ensure_extensions_loaded(true).await?;
        Ok(self
            .agent
            .get_prompt(&self.session_id, name, arguments)
            .await?
            .messages)
    }

    /// Process a single message and get the response
    pub(crate) async fn process_message(
        &mut self,
        message: Message,
        cancel_token: CancellationToken,
        interactive: bool,
    ) -> Result<()> {
        self.ensure_extensions_loaded(interactive).await?;
        let cancel_token = cancel_token.clone();
        self.push_message(message);
        self.process_agent_response(interactive, cancel_token)
            .await?;
        Ok(())
    }

    /// Start an interactive session, optionally with an initial message
    pub async fn interactive(&mut self, prompt: Option<String>) -> Result<()> {
        let banners = self
            .agent
            .emit_hook_with_banners(goose::hooks::HookEvent::SessionStart, &self.session_id)
            .await;
        if !banners.is_empty() {
            output::display_banner(&banners);
        }

        let result = self.run_interactive(prompt).await;

        self.agent
            .emit_hook(goose::hooks::HookEvent::SessionEnd, &self.session_id)
            .await;

        if result.is_ok() {
            println!(
                "\n  {} {}",
                console::style("●").red(),
                console::style(format!("session closed · {}", self.session_id)).dim()
            );
        }

        result
    }

    async fn run_interactive(&mut self, prompt: Option<String>) -> Result<()> {
        if let Some(prompt) = prompt {
            let msg = Message::user().with_text(&prompt);
            self.process_message(msg, CancellationToken::default(), true)
                .await?;
        } else if self
            .extension_loading
            .as_ref()
            .is_some_and(|h| !h.is_finished())
        {
            self.loading_announced = true;
            output::show_loading_extensions_background();
        }

        // The completion cache starts empty: populating it here would issue
        // prompts/list to every connected extension, so a slow responder could
        // block the prompt right after we announced loading continues in the
        // background. The loader refreshes the shared cache when it finishes.
        let mut editor = self.create_editor()?;
        let history_manager = HistoryManager::new();
        history_manager.load(&mut editor);

        loop {
            if self
                .extension_loading
                .as_ref()
                .is_some_and(|h| h.is_finished())
            {
                self.ensure_extensions_loaded(true).await?;
            }

            self.display_context_usage().await?;

            let conversation_strings: Vec<String> = self
                .messages
                .user_visible_messages()
                .iter()
                .map(|msg| {
                    let role = match msg.role {
                        rmcp::model::Role::User => "User",
                        rmcp::model::Role::Assistant => "Assistant",
                    };
                    format!("## {}: {}", role, msg.as_concat_text())
                })
                .collect();

            output::run_status_hook("waiting");
            let input = input::get_input(&mut editor, Some(&conversation_strings))?;
            if matches!(input, InputResult::Exit) {
                break;
            }
            self.handle_input(input, &history_manager, &mut editor, &conversation_strings)
                .await?;
        }

        Ok(())
    }

    fn create_editor(
        &self,
    ) -> Result<rustyline::Editor<GooseCompleter, rustyline::history::DefaultHistory>> {
        let builder =
            rustyline::Config::builder().completion_type(rustyline::CompletionType::Circular);
        let builder = match self.edit_mode {
            Some(mode) => builder.edit_mode(mode),
            None => builder.edit_mode(EditMode::Emacs),
        };
        let config = builder.build();
        let mut editor =
            rustyline::Editor::<GooseCompleter, rustyline::history::DefaultHistory>::with_config(
                config,
            )?;
        let completer = GooseCompleter::new(self.completion_cache.clone());
        editor.set_helper(Some(completer));
        Ok(editor)
    }

    async fn handle_input(
        &mut self,
        input: InputResult,
        history: &HistoryManager,
        editor: &mut rustyline::Editor<GooseCompleter, rustyline::history::DefaultHistory>,
        conversation_messages: &[String],
    ) -> Result<()> {
        // The REPL's loading gate: every command, including future ones, waits
        // here until background extension loading has finished.
        self.ensure_extensions_loaded(true).await?;
        match input {
            InputResult::Message(content) => {
                self.handle_message_input(&content, history, editor).await?;
            }
            InputResult::Exit => unreachable!("Exit is handled in the main loop"),
            InputResult::AddExtension(cmd) => {
                history.save(editor);
                match self.add_extension(cmd.clone()).await {
                    Ok(_) => output::render_extension_success(&cmd),
                    Err(e) => output::render_extension_error(&cmd, &e.to_string()),
                }
            }
            InputResult::AddBuiltin(names) => {
                history.save(editor);
                match self.add_builtin(names.clone()).await {
                    Ok(_) => output::render_builtin_success(&names),
                    Err(e) => output::render_builtin_error(&names, &e.to_string()),
                }
            }
            InputResult::ToggleTheme => {
                history.save(editor);
                self.handle_toggle_theme();
            }
            InputResult::ToggleFullToolOutput => {
                history.save(editor);
                self.handle_toggle_full_tool_output();
            }
            InputResult::SelectTheme(theme_name) => {
                history.save(editor);
                self.handle_select_theme(&theme_name);
            }
            InputResult::Retry => {}
            InputResult::ListPrompts(extension) => {
                history.save(editor);
                match self.list_prompts(extension).await {
                    Ok(prompts) => output::render_prompts(&prompts),
                    Err(e) => output::render_error(&e.to_string()),
                }
            }
            InputResult::GooseMode(mode) => {
                history.save(editor);
                self.handle_goose_mode(&mode).await?;
            }
            InputResult::Model(options) => {
                history.save(editor);
                self.handle_model(options).await?;
            }
            InputResult::Plan(options) => {
                self.handle_plan_mode(options).await?;
            }
            InputResult::EndPlan => {
                self.run_mode = RunMode::Normal;
                output::render_exit_plan_mode();
            }
            InputResult::Clear => {
                history.save(editor);
                self.handle_clear().await?;
            }
            InputResult::New => {
                history.save(editor);
                self.handle_new().await?;
            }
            InputResult::PromptCommand(opts) => {
                history.save(editor);
                self.handle_prompt_command(opts).await?;
            }
            InputResult::Compact => {
                history.save(editor);
                self.handle_compact().await?;
            }
            InputResult::Edit(prefill) => {
                history.save(editor);
                match crate::session::editor::resolve_editor_command() {
                    Some(editor_cmd) => {
                        let messages: Vec<&str> =
                            conversation_messages.iter().map(|s| s.as_str()).collect();
                        match crate::session::editor::get_editor_input(
                            &editor_cmd,
                            &messages,
                            prefill.as_deref(),
                        ) {
                            Ok((message, true)) => {
                                editor.add_history_entry(message.as_str())?;
                                history.save(editor);
                                self.handle_message_input(&message, history, editor).await?;
                            }
                            Ok((_, false)) => {}
                            Err(e) => {
                                output::render_error(&format!("Failed to open editor: {}", e));
                            }
                        }
                    }
                    None => {
                        output::render_error(
                            "No editor found. Set one with:\n  \
                                 goose configure set goose_prompt_editor \"vim\"\n  \
                                 or set $VISUAL or $EDITOR in your shell.",
                        );
                    }
                }
            }
            InputResult::LoadSkills(names) => {
                history.save(editor);
                self.handle_load_skills(&names).await?;
            }
            InputResult::ListSkills => {
                history.save(editor);
                self.handle_list_skills().await?;
            }
        }
        Ok(())
    }

    async fn handle_message_input(
        &mut self,
        content: &str,
        history: &HistoryManager,
        editor: &mut rustyline::Editor<GooseCompleter, rustyline::history::DefaultHistory>,
    ) -> Result<()> {
        match self.run_mode {
            RunMode::Normal => {
                history.save(editor);
                self.push_message(Message::user().with_text(content));

                let _provider = self.agent.provider().await?;

                println!();
                output::run_status_hook("thinking");
                output::show_thinking();
                let start_time = Instant::now();
                self.process_agent_response(true, CancellationToken::default())
                    .await?;
                output::hide_thinking();

                let elapsed = start_time.elapsed();
                let elapsed_str = format_elapsed_time(elapsed);
                println!("{}", console::style(format!("  ⏱ {}", elapsed_str)).dim());
            }
            RunMode::Plan => {
                let mut plan_messages = self.messages.clone();
                plan_messages.push(Message::user().with_text(content));
                let (reasoner, reasoner_model_config) = get_reasoner().await?;
                self.plan_with_reasoner_model(plan_messages, reasoner, reasoner_model_config)
                    .await?;
            }
        }
        Ok(())
    }

    fn handle_toggle_theme(&self) {
        let current = output::get_theme();
        let new_theme = match current {
            output::Theme::Ansi => {
                println!("Switching to Light theme");
                output::Theme::Light
            }
            output::Theme::Light => {
                println!("Switching to Dark theme");
                output::Theme::Dark
            }
            output::Theme::Dark => {
                println!("Switching to Ansi theme");
                output::Theme::Ansi
            }
        };
        output::set_theme(new_theme);
    }

    fn handle_select_theme(&self, theme_name: &str) {
        let new_theme = match theme_name {
            "light" => {
                println!("Switching to Light theme");
                output::Theme::Light
            }
            "dark" => {
                println!("Switching to Dark theme");
                output::Theme::Dark
            }
            "ansi" => {
                println!("Switching to Ansi theme");
                output::Theme::Ansi
            }
            _ => output::Theme::Dark,
        };
        output::set_theme(new_theme);
    }

    fn handle_toggle_full_tool_output(&self) {
        let enabled = output::toggle_full_tool_output();
        if enabled {
            println!(
                "{}",
                console::style(
                    "✓ Full tool output enabled - tool parameters will no longer be truncated"
                )
                .green()
            );
        } else {
            println!(
                "{}",
                console::style(
                    "✓ Full tool output disabled - tool parameters will be truncated to fit terminal width"
                )
                .dim()
            );
        }
    }

    async fn handle_goose_mode(&self, mode: &str) -> Result<()> {
        let config = Config::global();
        let mode = match GooseMode::from_str(&mode.to_lowercase()) {
            Ok(mode) => mode,
            Err(_) => {
                output::render_error(&format!(
                    "Invalid mode '{mode}'. Mode must be one of: {}",
                    GooseMode::VARIANTS.join(", ")
                ));
                return Ok(());
            }
        };
        self.agent.update_goose_mode(mode, &self.session_id).await?;
        config.set_goose_mode(mode)?;
        output::goose_mode_message(&format!("Goose mode set to '{mode}'"));
        Ok(())
    }

    async fn handle_model(&mut self, options: input::ModelCommandOptions) -> Result<()> {
        let provider = self.agent.provider().await?;
        let current_provider_name = provider.get_name().to_string();
        let current_model_config = self
            .agent
            .model_config_for_session(&self.session_id)
            .await?;
        let current_model_name = current_model_config.model_name.clone();

        if options.provider.is_none() && options.model.is_none() {
            output::goose_mode_message(&format!(
                "Current session model: '{}' (provider '{}')\n\
                 Tip: use '/model <name>' to switch model, or '/model --provider <name> [model]' to switch provider.",
                current_model_name, current_provider_name
            ));
            return Ok(());
        }

        let requested_provider = options
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let target_provider_name = requested_provider.unwrap_or(&current_provider_name);

        if options.provider.is_some() && requested_provider.is_none() {
            output::render_error("Provider name is required after '--provider'.");
            return Ok(());
        }

        let target_entry = match goose::providers::get_from_registry(target_provider_name).await {
            Ok(entry) => entry,
            Err(_) => {
                output::render_error(&format!(
                    "Unknown provider '{}'. Use tab-completion to see available providers.",
                    target_provider_name
                ));
                return Ok(());
            }
        };

        if target_provider_name.ends_with("-acp") {
            output::render_error(
                "Session model switching is not supported for ACP providers in the CLI.",
            );
            return Ok(());
        }

        if provider.manages_own_context() {
            output::render_error(&format!(
                "Session model or provider switching is not supported for provider '{}' because it manages its own conversation context.",
                current_provider_name
            ));
            return Ok(());
        }

        if options
            .model
            .as_deref()
            .is_some_and(|model| model.split_whitespace().count() > 1)
        {
            output::render_error("Unexpected arguments after model name.");
            return Ok(());
        }

        let target_model_name = match options.model.as_deref().map(str::trim) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => {
                if target_provider_name == current_provider_name {
                    current_model_name.clone()
                } else {
                    let known: Vec<&str> = target_entry
                        .metadata()
                        .known_models
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect();
                    if known.contains(&current_model_name.as_str()) {
                        current_model_name.clone()
                    } else {
                        target_entry.metadata().default_model.clone()
                    }
                }
            }
        };

        let new_model_config = build_switched_model_config(
            target_provider_name,
            &target_model_name,
            &current_model_config,
        )?;

        let configured_effort = Config::global().get_goose_thinking_effort();
        let new_effort = new_model_config.thinking_effort().or(configured_effort);
        let current_effort = current_model_config.thinking_effort().or(configured_effort);
        let provider_unchanged = target_provider_name == current_provider_name;
        if provider_unchanged
            && new_model_config.model_name == current_model_config.model_name
            && new_effort == current_effort
        {
            output::goose_mode_message(&format!(
                "Session already using model '{}' for provider '{}'",
                current_model_name, current_provider_name
            ));
            return Ok(());
        }

        let current_context_limit = goose::context_limit::get_context_limit(
            provider.as_ref(),
            &current_model_config.model_name,
        )
        .await?;

        let extensions = self.agent.get_extension_configs().await;
        let new_provider = match goose::providers::create(target_provider_name, extensions).await {
            Ok(p) => p,
            Err(e) => {
                output::render_error(&format!(
                    "Cannot switch to provider '{}': {}\n\
                         Set credentials via `goose configure` or the appropriate environment variable.\n\
                         Session continues with current provider '{}'.",
                    target_provider_name, e, current_provider_name
                ));
                return Ok(());
            }
        };

        if new_provider.manages_own_context() {
            output::render_error(&format!(
                "Session provider switching is not supported for '{}' because it manages its own conversation context.",
                target_provider_name
            ));
            return Ok(());
        }

        let new_context_limit = goose::context_limit::get_context_limit(
            new_provider.as_ref(),
            &new_model_config.model_name,
        )
        .await?;
        if new_context_limit < current_context_limit {
            eprintln!(
                "{}",
                console::style(format!(
                    "Warning: '{}' has a smaller context window ({} tokens) than the current session ({} tokens). You may need to use /compact.",
                    target_model_name, new_context_limit, current_context_limit
                ))
                .yellow()
            );
        }

        self.agent
            .update_provider(new_provider, new_model_config, &self.session_id)
            .await?;

        let mode = self.agent.goose_mode().await;
        self.agent.update_goose_mode(mode, &self.session_id).await?;

        self.update_completion_cache().await?;

        if provider_unchanged {
            output::goose_mode_message(&format!(
                "Session model switched from '{}' to '{}' for provider '{}'",
                current_model_name, target_model_name, current_provider_name
            ));
        } else {
            output::goose_mode_message(&format!(
                "Session switched from provider '{}' / model '{}' to provider '{}' / model '{}'",
                current_provider_name, current_model_name, target_provider_name, target_model_name
            ));
        }
        Ok(())
    }

    async fn handle_plan_mode(&mut self, options: input::PlanCommandOptions) -> Result<()> {
        self.run_mode = RunMode::Plan;
        output::render_enter_plan_mode();

        if options.message_text.is_empty() {
            return Ok(());
        }

        let mut plan_messages = self.messages.clone();
        plan_messages.push(Message::user().with_text(&options.message_text));

        let (reasoner, reasoner_model_config) = get_reasoner().await?;
        self.plan_with_reasoner_model(plan_messages, reasoner, reasoner_model_config)
            .await
    }

    async fn handle_clear(&mut self) -> Result<()> {
        let provider = self.agent.provider().await?;
        if provider.manages_own_context() {
            output::render_error(&context_management_unsupported_message(
                "clear",
                provider.get_name(),
            ));
            return Ok(());
        }

        if let Err(e) = self
            .agent
            .config
            .session_manager
            .replace_conversation(&self.session_id, &Conversation::default())
            .await
        {
            output::render_error(&format!("Failed to clear session: {}", e));
            return Ok(());
        }

        if let Err(e) = self
            .agent
            .config
            .session_manager
            .update(&self.session_id)
            .usage(goose_providers::conversation::token_usage::Usage::new(
                Some(0),
                Some(0),
                Some(0),
            ))
            .apply()
            .await
        {
            output::render_error(&format!("Failed to reset token counts: {}", e));
            return Ok(());
        }

        self.messages.clear();
        tracing::info!("Chat context cleared by user.");
        output::render_message(
            &Message::assistant().with_text("Chat context cleared.\n"),
            self.debug,
        );
        Ok(())
    }

    async fn handle_new(&mut self) -> Result<()> {
        let provider = self.agent.provider().await?;
        if provider.manages_own_context() {
            output::render_error(&format!(
                "Starting a new session is not supported for provider '{}' because it manages its own conversation context.",
                provider.get_name()
            ));
            return Ok(());
        }

        // The background loader pins the session id it was spawned with, and the
        // handle_input gate has already drained it, so extensions can be torn
        // down and re-added under a new session id without racing an in-flight
        // load.

        let new_session_id = match self.prepare_successor_session().await {
            Ok(id) => id,
            Err(e) => {
                output::render_error(&format!("Failed to start a new session: {}", e));
                return Ok(());
            }
        };

        let extension_configs = self.agent.get_extension_configs().await;

        self.agent
            .emit_hook(goose::hooks::HookEvent::SessionEnd, &self.session_id)
            .await;

        self.agent.discard_pending_steers(&self.session_id).await;

        self.session_id = new_session_id;
        self.messages.clear();
        self.run_mode = RunMode::Normal;
        self.agent.set_goal(None).await;
        self.agent.set_grind(None).await;

        if let Err(e) = self
            .agent
            .update_goose_mode(self.agent.goose_mode().await, &self.session_id)
            .await
        {
            output::render_error(&format!("Failed to apply the current mode: {}", e));
        }

        if !extension_configs.is_empty() {
            output::goose_mode_message("Restarting extensions for the new session...");
        }

        // MCP clients pin themselves to the first session id they see a request for, so
        // extensions must be torn down and re-added under the new session id.
        for name in self.agent.list_extensions().await {
            if let Err(e) = self.agent.remove_extension(&name, &self.session_id).await {
                output::render_extension_error(&name, &e.to_string());
            }
        }

        let mut unavailable = Vec::new();
        for config in extension_configs {
            let name = config.name();
            if let Err(e) = self.agent.add_extension(config, &self.session_id).await {
                output::render_extension_error(&name, &e.to_string());
                unavailable.push(name);
            }
        }

        if let Err(e) = self.update_completion_cache().await {
            output::render_error(&format!("Failed to refresh completions: {}", e));
        }

        let mut started = format!("Started a new session · {}\n", self.session_id);
        if !unavailable.is_empty() {
            started.push_str(&format!(
                "Continuing without these extensions: {}\n",
                unavailable.join(", ")
            ));
        }
        output::render_message(&Message::assistant().with_text(started), self.debug);
        Ok(())
    }

    async fn prepare_successor_session(&self) -> Result<String> {
        let session_manager = &self.agent.config.session_manager;
        let old_session = session_manager.get_session(&self.session_id, false).await?;
        let new_session_id =
            create_successor_session(session_manager, &old_session, self.agent.goose_mode().await)
                .await?;
        self.agent.persist_extension_state(&new_session_id).await?;
        Ok(new_session_id)
    }

    async fn handle_load_skills(&mut self, names: &[String]) -> Result<()> {
        // NOTE: We don't validate the skill names here because the load_skill tool will
        // handle that and provide feedback to the user if any skill names are invalid.
        let message = format!(
            "Use the load_skill tool to load the following skills: {}.",
            names
                .iter()
                .map(|n| format!("\"{}\"", n))
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.push_message(Message::user().with_text(&message));
        output::show_thinking();
        let result = self
            .process_agent_response(true, CancellationToken::default())
            .await;
        output::hide_thinking();
        result?;

        Ok(())
    }

    async fn handle_list_skills(&mut self) -> Result<()> {
        use comfy_table::{presets, Cell, ContentArrangement, Table};
        use goose::custom_requests::SourceType;
        use goose::skills::list_installed_skills;
        let cwd = std::env::current_dir().unwrap_or_default();
        let skills = list_installed_skills(Some(&cwd));

        if skills.is_empty() {
            println!("{}", console::style("No skills available.").yellow());
            return Ok(());
        }

        let mut table = Table::new();
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.load_style(presets::ASCII_FULL);
        table.set_header(vec!["Skill", "Location", "Description"]);

        let mut sorted_skills = skills;
        sorted_skills.sort_by(|a, b| a.name.cmp(&b.name));

        for skill in &sorted_skills {
            let location = if skill.source_type == SourceType::BuiltinSkill {
                "built-in"
            } else if skill.global {
                "global"
            } else {
                "project"
            };
            table.add_row(vec![
                Cell::new(&skill.name),
                Cell::new(location),
                Cell::new(&skill.description),
            ]);
        }

        println!("{table}");
        Ok(())
    }

    async fn handle_compact(&mut self) -> Result<()> {
        let provider = self.agent.provider().await?;
        if provider.manages_own_context() {
            output::render_error(&context_management_unsupported_message(
                "compact",
                provider.get_name(),
            ));
            return Ok(());
        }

        let prompt = "Are you sure you want to compact this conversation? This will condense the message history.";
        let should_summarize = match cliclack::confirm(prompt).initial_value(true).interact() {
            Ok(choice) => choice,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    false
                } else {
                    return Err(e.into());
                }
            }
        };

        if should_summarize {
            self.push_message(Message::user().with_text(COMPACT_TRIGGERS[0]));
            output::show_thinking();
            self.process_agent_response(true, CancellationToken::default())
                .await?;
            output::hide_thinking();
        } else {
            println!("{}", console::style("Compaction cancelled.").yellow());
        }
        Ok(())
    }

    async fn plan_with_reasoner_model(
        &mut self,
        plan_messages: Conversation,
        reasoner: Arc<dyn Provider>,
        model_config: goose_providers::model::ModelConfig,
    ) -> Result<(), anyhow::Error> {
        let plan_prompt = self.agent.get_plan_prompt(&self.session_id).await?;
        let provider_messages = planner_provider_messages(&plan_messages);
        output::show_thinking();
        let (plan_response, _usage) = goose::session_context::with_session_id(
            Some(self.session_id.clone()),
            reasoner.complete(
                &model_config,
                &plan_prompt,
                provider_messages.messages(),
                &[],
            ),
        )
        .await?;
        let classifier_text = planner_classification_text(&plan_response);
        let plan_response = plan_response.user_visible_content();
        output::render_message(&plan_response, self.debug);
        output::hide_thinking();
        let classifier_text = classifier_text?;
        anyhow::ensure!(
            !plan_response.content.is_empty(),
            "Planner returned no user-visible content"
        );
        let planner_response_type = classify_planner_response(
            &self.session_id,
            classifier_text,
            self.agent.provider().await?,
            self.agent
                .model_config_for_session(&self.session_id)
                .await?,
        )
        .await?;

        output::emit_attention_bell();

        match planner_response_type {
            PlannerResponseType::Plan => {
                println!();
                let should_act = match cliclack::confirm(
                    "Do you want to clear message history & act on this plan?",
                )
                .initial_value(true)
                .interact()
                {
                    Ok(choice) => choice,
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            false // If interrupted, set should_act to false
                        } else {
                            return Err(e.into());
                        }
                    }
                };
                if should_act {
                    output::render_act_on_plan();
                    self.run_mode = RunMode::Normal;
                    // set goose mode: auto if that isn't already the case
                    let config = Config::global();
                    let curr_goose_mode = config.get_goose_mode().unwrap_or_default();
                    if curr_goose_mode != GooseMode::Auto {
                        config.set_goose_mode(GooseMode::Auto).unwrap();
                    }

                    // clear the messages before acting on the plan
                    self.messages.clear();
                    // add the plan response as a user message
                    let plan_message = Message::user().with_text(plan_response.as_concat_text());
                    self.push_message(plan_message);
                    // act on the plan
                    output::show_thinking();
                    self.process_agent_response(true, CancellationToken::default())
                        .await?;
                    output::hide_thinking();

                    // Reset run & goose mode
                    if curr_goose_mode != GooseMode::Auto {
                        config.set_goose_mode(curr_goose_mode)?;
                    }
                } else {
                    // add the plan response (assistant message) & carry the conversation forward
                    // in the next round, the user might wanna slightly modify the plan
                    self.push_message(plan_response);
                }
            }
            PlannerResponseType::ClarifyingQuestions => {
                // add the plan response (assistant message) & carry the conversation forward
                // in the next round, the user will answer the clarifying questions
                self.push_message(plan_response);
            }
        }

        Ok(())
    }

    /// Process a single message and exit
    pub async fn headless(&mut self, prompt: String) -> Result<()> {
        let message = Message::user().with_text(&prompt);
        let result = self
            .process_message(message, CancellationToken::default(), false)
            .await;
        self.agent
            .emit_hook(goose::hooks::HookEvent::SessionEnd, &self.session_id)
            .await;
        result?;
        Ok(())
    }

    async fn process_agent_response(
        &mut self,
        interactive: bool,
        cancel_token: CancellationToken,
    ) -> Result<()> {
        let is_json_mode = self.output_format == "json";
        let is_stream_json_mode = self.output_format == "stream-json";

        let session_config = SessionConfig {
            id: self.session_id.clone(),
            schedule_id: self.scheduled_job_id.clone(),
            max_turns: self.max_turns,
            retry_config: self.retry_config.clone(),
        };
        let user_message = self
            .messages
            .last()
            .ok_or_else(|| anyhow::anyhow!("No user message"))?;
        let cancel_token_interrupt = cancel_token.clone();
        let handle = tokio::spawn(async move {
            if ctrl_c().await.is_ok() {
                cancel_token_interrupt.cancel();
            }
        });
        let _drop_handle = AbortOnDropHandle::new(handle);

        let mut stream = self
            .agent
            .reply(
                user_message.clone(),
                session_config.clone(),
                Some(cancel_token.clone()),
            )
            .await?;

        let mut progress_bars = output::McpSpinners::new();
        let cancel_token_clone = cancel_token.clone();
        let mut markdown_buffer = streaming_buffer::MarkdownBuffer::new();
        let mut prompted_credits_urls: HashSet<String> = HashSet::new();
        let mut thinking_header_shown = false;
        let run_started = Instant::now();
        let mut first_token_at: Option<Instant> = None;
        let mut last_usage: Option<ProviderUsage> = None;
        let mut stream_error = None;

        use futures::StreamExt;
        loop {
            tokio::select! {
                result = stream.next() => {
                    match result {
                        Some(Ok(AgentEvent::Message(message))) => {
                            if first_token_at.is_none() && message_has_text(&message) {
                                first_token_at = Some(Instant::now());
                            }
                            if let Some(confirmation_request) = find_tool_confirmation(&message) {
                                let selected_permission = if interactive {
                                    prompt_tool_confirmation(&confirmation_request)?
                                } else {
                                    // Non-interactive/headless mode: refuse to run in
                                    // Approve/SmartApprove modes since auto-allowing would
                                    // bypass the safety contract those modes are meant to enforce.
                                    let config = Config::global();
                                    let goose_mode = config.get_goose_mode().unwrap_or(GooseMode::Auto);
                                    if goose_mode == GooseMode::Approve || goose_mode == GooseMode::SmartApprove {
                                        cancel_token_clone.cancel();
                                        drop(stream);
                                        return Err(anyhow::anyhow!(
                                            "Tool approval required in non-interactive mode with GooseMode::{goose_mode}. \
                                             This is an invalid configuration — Approve/SmartApprove modes require an \
                                             interactive terminal. Use GooseMode::Auto for headless sessions."
                                        ));
                                    }
                                    tracing::warn!(
                                        "Tool confirmation required in non-interactive mode, auto-allowing"
                                    );
                                    Permission::AllowOnce
                                };

                                let cancelled_by_user = selected_permission == Permission::Cancel;
                                if cancelled_by_user {
                                    output::render_text("Tool call cancelled. Returning to chat...", Some(Color::Yellow), true);
                                }
                                self.agent
                                    .submit_tool_confirmation(
                                        &self.session_id,
                                        &confirmation_request.id,
                                        selected_permission,
                                    )
                                    .await?;
                                if cancelled_by_user {
                                    let mut response_message = Message::user();
                                    response_message.content.push(MessageContent::tool_response(
                                        confirmation_request.id,
                                        Err(ErrorData {
                                            code: ErrorCode::INVALID_REQUEST,
                                            message: std::borrow::Cow::from(
                                                "Tool call cancelled by user",
                                            ),
                                            data: None,
                                        }),
                                    ));
                                    self.agent
                                        .config
                                        .session_manager
                                        .add_message(&self.session_id, &response_message)
                                        .await?;
                                    self.messages.push(response_message);
                                    cancel_token_clone.cancel();
                                    drop(stream);
                                    break;
                                }
                            } else if let Some((elicitation_id, elicitation_message, schema)) = find_elicitation_request(&message) {
                                if !interactive {
                                    // Non-interactive/headless mode: cannot collect user input
                                    tracing::warn!(
                                        "Elicitation requested in non-interactive mode, cancelling"
                                    );
                                    cancel_token_clone.cancel();
                                    drop(stream);
                                    return Err(anyhow::anyhow!(
                                        "Elicitation requested but no interactive terminal is available to collect user input"
                                    ));
                                }

                                output::hide_thinking();
                                let _ = progress_bars.hide();

                                match elicitation::collect_elicitation_input(&elicitation_message, &schema) {
                                    Ok(input) => {
                                        match &input.action {
                                            ElicitationAction::Decline => {
                                                output::render_text("Information request declined.", Some(Color::Yellow), true);
                                            }
                                            ElicitationAction::Cancel => {
                                                output::render_text("Information request cancelled.", Some(Color::Yellow), true);
                                            }
                                            ElicitationAction::Accept => {}
                                            _ => {}
                                        }

                                        let should_cancel = input.action == ElicitationAction::Cancel;
                                        let action = input.action;
                                        let user_data_value = serde_json::to_value(input.user_data)
                                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                        let response_message = Message::user()
                                            .with_content(MessageContent::action_required_elicitation_response(
                                                elicitation_id,
                                                user_data_value,
                                                action,
                                            ))
                                            .with_visibility(false, true);
                                        self.messages.push(response_message.clone());
                                        // Elicitation responses return an empty stream - the response
                                        // unblocks the waiting tool call via ActionRequiredManager
                                        let _ = self.agent.reply(response_message, session_config.clone(), Some(cancel_token.clone())).await?;
                                        if should_cancel {
                                            cancel_token_clone.cancel();
                                            drop(stream);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        output::render_error(&format!("Failed to collect input: {}", e));
                                        cancel_token_clone.cancel();
                                        drop(stream);
                                        break;
                                    }
                                }
                            } else {
                                log_tool_metrics(&message, &self.messages);
                                self.messages.push(message.clone());

                                if interactive { output::hide_thinking() };
                                let _ = progress_bars.hide();

                                if is_stream_json_mode {
                                    emit_stream_event(&StreamEvent::Message { message: message.clone() });
                                } else if !is_json_mode {
                                    output::render_message_streaming(&message, &mut markdown_buffer, &mut thinking_header_shown, self.debug);
                                    maybe_open_credits_top_up_url(
                                        &message,
                                        interactive,
                                        &mut prompted_credits_urls,
                                    );
                                }
                            }
                        }
                        Some(Ok(AgentEvent::Usage(usage))) => {
                            last_usage = Some(usage);
                        }
                        Some(Ok(AgentEvent::MessageUsage { .. })) => {}
                        Some(Ok(AgentEvent::McpNotification((extension_id, notification)))) => {
                            handle_mcp_notification(
                                &extension_id,
                                &notification,
                                &mut progress_bars,
                                is_stream_json_mode,
                                interactive,
                                is_json_mode,
                                self.debug,
                            );
                        }
                        Some(Ok(AgentEvent::HistoryReplaced(updated_conversation))) => {
                            self.messages = updated_conversation;
                        }
                        Some(Err(e)) => {
                            if interactive || !is_stream_json_mode {
                                handle_agent_error(&e, is_stream_json_mode);
                            }
                            cancel_token_clone.cancel();
                            drop(stream);
                            if let Err(e) = self.handle_interrupted_messages(false).await {
                                eprintln!("Error handling interruption: {}", e);
                            } else if !is_stream_json_mode {
                                output::render_error(
                                    "The error above was an exception we were not able to handle.\n\
                                    These errors are often related to connection or authentication\n\
                                    We've removed the conversation up to the most recent user message\n\
                                    - depending on the error you may be able to continue",
                                );
                            }
                            stream_error = Some(e);
                            break;
                        }
                        None => break,
                    }
                }
                _ = cancel_token_clone.cancelled() => {
                    drop(stream);
                    if let Err(e) = self.handle_interrupted_messages(true).await {
                        eprintln!("Error handling interruption: {}", e);
                    }
                    break;
                }
            }
        }

        let terminal_error = headless_run_error(
            interactive,
            cancel_token_clone.is_cancelled(),
            stream_error,
            &self.messages,
        );
        if is_stream_json_mode {
            if let Some(error) = &terminal_error {
                emit_stream_event(&StreamEvent::Error {
                    error: error.to_string(),
                });
            }
        }

        if !is_json_mode && !is_stream_json_mode {
            output::flush_markdown_buffer_current_theme(&mut markdown_buffer);
        }

        if is_json_mode {
            let status = if terminal_error.is_some() {
                "error"
            } else {
                "completed"
            };
            let metadata = match self
                .agent
                .config
                .session_manager
                .get_session_usage_totals(&self.session_id)
                .await
            {
                Ok(totals) => JsonMetadata {
                    total_tokens: totals.accumulated_usage.total_tokens,
                    input_tokens: totals.accumulated_usage.input_tokens,
                    output_tokens: totals.accumulated_usage.output_tokens,
                    cache_read_input_tokens: totals.accumulated_usage.cache_read_input_tokens,
                    cache_write_input_tokens: totals.accumulated_usage.cache_write_input_tokens,
                    cost_usd: totals.accumulated_cost,
                    status: status.to_string(),
                },
                Err(_) => JsonMetadata {
                    total_tokens: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_input_tokens: None,
                    cache_write_input_tokens: None,
                    cost_usd: None,
                    status: status.to_string(),
                },
            };
            let json_output = JsonOutput {
                messages: self.messages.user_visible_messages(),
                metadata,
            };
            println!("{}", serde_json::to_string_pretty(&json_output)?);
        } else if is_stream_json_mode && terminal_error.is_none() {
            let totals = self
                .agent
                .config
                .session_manager
                .get_session_usage_totals(&self.session_id)
                .await
                .ok();
            let (
                total_tokens,
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_write_input_tokens,
                cost_usd,
            ) = match totals {
                Some(totals) => (
                    totals.accumulated_usage.total_tokens,
                    totals.accumulated_usage.input_tokens,
                    totals.accumulated_usage.output_tokens,
                    totals.accumulated_usage.cache_read_input_tokens,
                    totals.accumulated_usage.cache_write_input_tokens,
                    totals.accumulated_cost,
                ),
                None => (None, None, None, None, None, None),
            };
            emit_stream_event(&StreamEvent::Complete {
                total_tokens,
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_write_input_tokens,
                cost_usd,
            });
        } else if !is_stream_json_mode {
            println!();
            if self.stats {
                print_run_stats(run_started, first_token_at, last_usage.as_ref());
            }
        }

        if interactive {
            output::emit_attention_bell();
        }

        if let Some(error) = terminal_error {
            return Err(error);
        }

        Ok(())
    }

    async fn handle_interrupted_messages(&mut self, interrupt: bool) -> Result<()> {
        if interrupt {
            let mut cache = self.completion_cache.write().unwrap();
            cache.hint_status = HintStatus::Interrupted;
        }

        let tool_requests = self
            .messages
            .last()
            .filter(|msg| msg.role == rmcp::model::Role::Assistant)
            .map_or(Vec::new(), |msg| {
                msg.content
                    .iter()
                    .filter_map(|content| {
                        if let MessageContent::ToolRequest(req) = content {
                            Some((req.id.clone(), req.tool_call.clone()))
                        } else {
                            None
                        }
                    })
                    .collect()
            });

        let interrupt_prompt = "Yes — what would you like me to do?";

        if !tool_requests.is_empty() {
            let mut response_message = Message::user();

            let notification = if interrupt {
                "Interrupted by the user to make a correction".to_string()
            } else {
                "An uncaught error happened during tool use".to_string()
            };
            for (req_id, _) in &tool_requests {
                response_message.content.push(MessageContent::tool_response(
                    req_id.clone(),
                    Err(ErrorData {
                        code: ErrorCode::INTERNAL_ERROR,
                        message: std::borrow::Cow::from(notification.clone()),
                        data: None,
                    }),
                ));
            }
            self.push_message(response_message);
            self.push_message(Message::assistant().with_text(interrupt_prompt));
            output::render_message(
                &Message::assistant().with_text(interrupt_prompt),
                self.debug,
            );
        } else {
            while self.messages.last().is_some_and(Message::is_turn_context) {
                self.messages.pop();
            }
            if let Some(last_msg) = self.messages.last() {
                if last_msg.role == rmcp::model::Role::User {
                    match last_msg.content.first() {
                        Some(MessageContent::ToolResponse(_)) => {
                            self.push_message(Message::assistant().with_text(interrupt_prompt));
                            output::render_message(
                                &Message::assistant().with_text(interrupt_prompt),
                                self.debug,
                            );
                        }
                        Some(_) => {
                            self.messages.pop();
                            let assistant_msg = Message::assistant().with_text(interrupt_prompt);
                            self.push_message(assistant_msg.clone());
                            output::render_message(&assistant_msg, self.debug);
                        }
                        None => {
                            // Empty message content — nothing to do, just continue gracefully
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The session's single extension-loading gate: wait for the background
    /// loader and surface any failures.
    ///
    /// Entry points that can touch the agent or the extension set call this
    /// rather than gating handlers individually, so new commands inherit the
    /// gate automatically. `interactive` controls the waiting/ready
    /// indicators. The loader refreshes the shared completion cache when it
    /// finishes so completions become available while readline is active.
    async fn ensure_extensions_loaded(&mut self, interactive: bool) -> Result<()> {
        if let Some(handle) = self.extension_loading.take() {
            let was_in_progress = !handle.is_finished();
            if interactive && was_in_progress {
                output::show_waiting_for_extensions();
            }
            let failures = handle
                .await
                .map_err(|e| anyhow::anyhow!("Extension loading task failed: {}", e))??;
            output::show_extension_failures(&failures);

            if interactive && (was_in_progress || self.loading_announced) {
                output::show_extensions_ready();
            }
            self.loading_announced = false;
        }
        Ok(())
    }

    pub async fn update_completion_cache(&mut self) -> Result<()> {
        Self::refresh_completion_cache(&self.agent, &self.session_id, &self.completion_cache).await
    }

    async fn refresh_completion_cache(
        agent: &Agent,
        session_id: &str,
        completion_cache: &Arc<std::sync::RwLock<CompletionCache>>,
    ) -> Result<()> {
        let prompts = agent.list_extension_prompts(session_id).await;
        let all_providers = goose::providers::providers().await;
        let session_provider = agent.provider().await?.get_name().to_string();

        let provider_ids: Vec<String> = all_providers.iter().map(|(m, _)| m.name.clone()).collect();
        let inventory_models: HashMap<String, Vec<String>> = {
            let storage = SessionManager::instance().storage().clone();
            let inventory = ProviderInventoryService::new(storage);
            inventory
                .entries(&provider_ids)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|entry| {
                    let model_ids: Vec<String> =
                        entry.models.iter().map(|m| m.id.clone()).collect();
                    (entry.provider_id, model_ids)
                })
                .collect()
        };

        let config = Config::global();
        let configured_models: HashMap<String, String> = all_providers
            .iter()
            .filter_map(|(m, _)| {
                providers::get_provider_entry(config, &m.name)
                    .map(|entry| (m.name.clone(), entry.model))
                    .filter(|(_, model)| !model.is_empty())
            })
            .collect();

        let mut cache = completion_cache.write().unwrap();
        cache.prompts.clear();
        cache.prompt_info.clear();

        for (extension, prompt_list) in prompts {
            let names: Vec<String> = prompt_list.iter().map(|p| p.name.clone()).collect();
            cache.prompts.insert(extension.clone(), names);

            for prompt in prompt_list {
                cache.prompt_info.insert(
                    prompt.name.clone(),
                    output::PromptInfo {
                        name: prompt.name.clone(),
                        description: prompt.description.clone(),
                        arguments: prompt.arguments.clone(),
                        extension: Some(extension.clone()),
                    },
                );
            }
        }

        cache.provider_names = all_providers.iter().map(|(m, _)| m.name.clone()).collect();
        cache.current_session_provider = session_provider;
        cache.provider_models.clear();
        for (metadata, _) in &all_providers {
            let mut models: Vec<String> = metadata
                .known_models
                .iter()
                .map(|m| m.name.clone())
                .collect();

            if let Some(inv_models) = inventory_models.get(&metadata.name) {
                for model_id in inv_models {
                    if !models.contains(model_id) {
                        models.push(model_id.clone());
                    }
                }
            }

            if let Some(model) = configured_models.get(&metadata.name) {
                if !models.contains(model) {
                    models.push(model.clone());
                }
            }

            cache.provider_models.insert(metadata.name.clone(), models);
        }

        cache.last_updated = Instant::now();
        Ok(())
    }

    /// Invalidate the completion cache
    /// This should be called when extensions are added or removed
    async fn invalidate_completion_cache(&self) {
        let mut cache = self.completion_cache.write().unwrap();
        cache.prompts.clear();
        cache.prompt_info.clear();
        cache.last_updated = Instant::now();
    }

    pub fn message_history(&self) -> Conversation {
        self.messages.clone()
    }

    /// Render all past messages from the session history
    pub fn render_message_history(&self) {
        let messages = self.messages.user_visible_messages();
        if messages.is_empty() {
            return;
        }

        println!(
            "\n  {} {}",
            console::style("↻").cyan(),
            console::style(format!("{} messages restored", messages.len())).dim()
        );

        // Render each message
        for message in &messages {
            output::render_message(message, self.debug);
        }

        println!();
    }

    pub async fn get_session(&self) -> Result<goose::session::Session> {
        self.agent
            .config
            .session_manager
            .get_session(&self.session_id, false)
            .await
    }

    pub async fn get_total_token_usage(&self) -> Result<Option<i32>> {
        let metadata = self.get_session().await?;
        Ok(metadata.accumulated_usage.total_tokens)
    }

    /// Display enhanced context usage with session totals
    pub async fn display_context_usage(&self) -> Result<()> {
        let provider = self.agent.provider().await?;
        let model_config = self
            .agent
            .model_config_for_session(&self.session_id)
            .await?;
        let context_limit =
            goose::context_limit::get_context_limit(provider.as_ref(), &model_config.model_name)
                .await?;

        let config = Config::global();
        let show_cost = config
            .get_param::<bool>("GOOSE_CLI_SHOW_COST")
            .unwrap_or(false);

        let provider_name = config
            .get_goose_provider()
            .unwrap_or_else(|_| "unknown".to_string());

        match self.get_session().await {
            Ok(metadata) => {
                let total_tokens = metadata.usage.total_tokens.unwrap_or(0) as usize;

                output::display_context_usage(total_tokens, context_limit);

                if show_cost {
                    output::display_cost_usage(
                        &provider_name,
                        &model_config.model_name,
                        &metadata.usage,
                    );
                }
            }
            Err(_) => {
                output::display_context_usage(0, context_limit);
            }
        }

        Ok(())
    }

    /// Handle prompt command execution
    async fn handle_prompt_command(&mut self, opts: input::PromptCommandOptions) -> Result<()> {
        // name is required
        if opts.name.is_empty() {
            output::render_error("Prompt name argument is required");
            return Ok(());
        }

        if opts.info {
            match self.get_prompt_info(&opts.name).await? {
                Some(info) => output::render_prompt_info(&info),
                None => output::render_error(&format!("Prompt '{}' not found", opts.name)),
            }
        } else {
            // Convert the arguments HashMap to a Value
            let arguments = serde_json::to_value(opts.arguments)
                .map_err(|e| anyhow::anyhow!("Failed to serialize arguments: {}", e))?;

            match self.get_prompt(&opts.name, arguments).await {
                Ok(messages) => {
                    let start_len = self.messages.len();
                    let mut valid = true;
                    let num_messages = messages.len();
                    for (i, prompt_message) in messages.into_iter().enumerate() {
                        let msg = Message::from(prompt_message);
                        // ensure we get a User - Assistant - User type pattern
                        let expected_role = if i % 2 == 0 {
                            rmcp::model::Role::User
                        } else {
                            rmcp::model::Role::Assistant
                        };

                        if msg.role != expected_role {
                            output::render_error(&format!(
                                "Expected {:?} message at position {}, but found {:?}",
                                expected_role, i, msg.role
                            ));
                            valid = false;
                            // get rid of everything we added to messages
                            self.messages.truncate(start_len);
                            break;
                        }

                        if msg.role == rmcp::model::Role::User {
                            output::render_message(&msg, self.debug);
                        }
                        self.push_message(msg);
                    }

                    if valid {
                        if num_messages > 1 {
                            for i in 0..(num_messages - 1) {
                                let msg = &self.messages.messages()[start_len + i];
                                self.agent
                                    .config
                                    .session_manager
                                    .add_message(&self.session_id, msg)
                                    .await?;
                            }
                        }

                        output::show_thinking();
                        self.process_agent_response(true, CancellationToken::default())
                            .await?;
                        output::hide_thinking();
                    }
                }
                Err(e) => output::render_error(&e.to_string()),
            }
        }

        Ok(())
    }

    fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }
}

async fn create_successor_session(
    session_manager: &SessionManager,
    old_session: &goose::session::Session,
    goose_mode: GooseMode,
) -> Result<String> {
    let new_session = session_manager
        .create_session(
            old_session.working_dir.clone(),
            "CLI Session".to_string(),
            old_session.session_type,
            goose_mode,
        )
        .await?;

    let mut builder = session_manager
        .update(&new_session.id)
        .recipe(old_session.recipe.clone())
        .user_recipe_values(old_session.user_recipe_values.clone());

    if let Some(provider_name) = old_session.provider_name.clone() {
        builder = builder.provider_name(provider_name);
    }
    if let Some(model_config) = old_session.model_config.clone() {
        builder = builder.model_config(model_config);
    }
    if let Some(project_id) = old_session.project_id.clone() {
        builder = builder.project_id(Some(project_id));
    }

    builder.apply().await?;

    Ok(new_session.id)
}

fn message_has_text(message: &Message) -> bool {
    message.content.iter().any(
        |content| matches!(content, MessageContent::Text(text) if !text.text.trim().is_empty()),
    )
}

fn print_run_stats(
    run_started: Instant,
    first_token_at: Option<Instant>,
    usage: Option<&ProviderUsage>,
) {
    let elapsed = run_started.elapsed();
    let stats = usage.and_then(|usage| usage.stats.as_ref());
    let generation_elapsed = stats
        .and_then(|stats| stats.elapsed_ms)
        .map(Duration::from_millis);
    let output_tokens = usage
        .and_then(|usage| usage.usage.output_tokens)
        .and_then(|tokens| usize::try_from(tokens).ok())
        .or_else(|| stats.and_then(|stats| stats.output_tokens));
    let tokens_per_second = output_tokens.map(|tokens| {
        let rate_elapsed = generation_elapsed.unwrap_or(elapsed);
        if rate_elapsed.as_secs_f64() > 0.0 {
            tokens as f64 / rate_elapsed.as_secs_f64()
        } else {
            0.0
        }
    });
    let model_load_ms = stats.and_then(|stats| stats.model_load_ms);
    let generation_time_to_first_token_ms = stats.and_then(|stats| stats.time_to_first_token_ms);

    eprintln!("\nStats:");
    if let Some(ms) = model_load_ms {
        eprintln!("  Model load: {:.2}s", ms as f64 / 1000.0);
    }
    if model_load_ms.is_some() {
        match generation_time_to_first_token_ms {
            Some(ms) => eprintln!(
                "  Generation time to first token: {:.2}s",
                ms as f64 / 1000.0
            ),
            None => eprintln!("  Generation time to first token: unavailable"),
        }
        match first_token_at {
            Some(first) => eprintln!(
                "  End-to-end time to first token: {:.2}s",
                first.duration_since(run_started).as_secs_f64()
            ),
            None => eprintln!("  End-to-end time to first token: unavailable"),
        }
    } else if let Some(ms) = generation_time_to_first_token_ms {
        eprintln!("  Time to first token: {:.2}s", ms as f64 / 1000.0);
    } else {
        match first_token_at {
            Some(first) => eprintln!(
                "  Time to first token: {:.2}s",
                first.duration_since(run_started).as_secs_f64()
            ),
            None => eprintln!("  Time to first token: unavailable"),
        }
    }
    match tokens_per_second {
        Some(rate) => eprintln!("  Tokens/sec: {:.2}", rate),
        None => eprintln!("  Tokens/sec: unavailable"),
    }
    if let Some(tokens) = output_tokens {
        eprintln!("  Output tokens: {tokens}");
    }

    if let Some(draft) = stats.and_then(|stats| stats.draft.as_ref()) {
        eprintln!("  Draft accept rate: {:.1}%", draft.accept_rate * 100.0);
        eprintln!(
            "  Draft tokens: {} accepted: {} target verified: {} rounds: {}",
            draft.draft_tokens, draft.accepted_tokens, draft.target_tokens, draft.rounds
        );
        if let Some(model) = &draft.model {
            eprintln!("  Draft model: {model}");
        }
    }
}

fn maybe_open_credits_top_up_url(
    message: &Message,
    interactive: bool,
    prompted_credits_urls: &mut HashSet<String>,
) {
    if !interactive || !std::io::stdout().is_terminal() {
        return;
    }

    let Some(url) = output::get_credits_top_up_url(message) else {
        return;
    };

    if !prompted_credits_urls.insert(url.clone()) {
        return;
    }

    let should_open = cliclack::confirm("Open the top-up URL in your browser?")
        .initial_value(false)
        .interact()
        .unwrap_or(false);

    if should_open && webbrowser::open(&url).is_err() {
        output::render_text(
            "Could not open browser automatically. Visit the URL above.",
            Some(Color::Yellow),
            true,
        );
    }
}

fn emit_stream_event(event: &StreamEvent) {
    if let Ok(json) = serde_json::to_string(event) {
        println!("{}", json);
    }
}

/// Prompt user for tool call confirmation, returns the Permission selected
fn prompt_tool_confirmation(request: &ToolConfirmationRequest) -> Result<Permission> {
    output::hide_thinking();
    output::emit_attention_bell();

    output::render_tool_confirmation(
        &request.tool_name,
        &request.arguments,
        request.prompt.as_deref(),
    );
    let prompt = if request.prompt.is_some() {
        "Do you allow this tool call?".to_string()
    } else {
        "Goose would like to call the above tool, do you allow?".to_string()
    };

    let permission_result = if request.prompt.is_none() {
        cliclack::select(prompt)
            .item(Permission::AllowOnce, "Allow", "Allow the tool call once")
            .item(
                Permission::AlwaysAllow,
                "Always Allow",
                "Always allow the tool call",
            )
            .item(Permission::DenyOnce, "Deny", "Deny the tool call")
            .item(
                Permission::Cancel,
                "Cancel",
                "Cancel the AI response and tool call",
            )
            .interact()
    } else {
        cliclack::select(prompt)
            .item(Permission::AllowOnce, "Allow", "Allow the tool call once")
            .item(Permission::DenyOnce, "Deny", "Deny the tool call")
            .item(
                Permission::Cancel,
                "Cancel",
                "Cancel the AI response and tool call",
            )
            .interact()
    };

    match permission_result {
        Ok(p) => Ok(p),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::Interrupted {
                Ok(Permission::Cancel)
            } else {
                Err(e.into())
            }
        }
    }
}

/// Extract tool confirmation request from a message
fn find_tool_confirmation(message: &Message) -> Option<ToolConfirmationRequest> {
    message.content.iter().find_map(|content| {
        if let MessageContent::ActionRequired(action) = content {
            if let ActionRequiredData::ToolConfirmation {
                id,
                tool_name,
                arguments,
                prompt,
            } = &action.data
            {
                return Some(ToolConfirmationRequest {
                    id: id.clone(),
                    tool_name: tool_name.clone(),
                    arguments: arguments.clone(),
                    prompt: prompt.clone(),
                });
            }
        }
        None
    })
}

/// Extract elicitation request from a message
fn find_elicitation_request(message: &Message) -> Option<(String, String, Value)> {
    message.content.iter().find_map(|content| {
        if let MessageContent::ActionRequired(action) = content {
            if let ActionRequiredData::Elicitation {
                id,
                message,
                requested_schema,
            } = &action.data
            {
                return Some((id.clone(), message.clone(), requested_schema.clone()));
            }
        }
        None
    })
}

/// Handle MCP notification event (logging or progress)
#[expect(deprecated)]
fn handle_mcp_notification(
    extension_id: &str,
    notification: &ServerNotification,
    progress_bars: &mut output::McpSpinners,
    is_stream_json_mode: bool,
    interactive: bool,
    is_json_mode: bool,
    debug: bool,
) {
    match notification {
        ServerNotification::LoggingMessageNotification(log_notif) => {
            if let Some(obj) = log_notif.params.data.as_object() {
                if obj.get("type").and_then(|v| v.as_str()) == Some(SUBAGENT_TOOL_REQUEST_TYPE) {
                    if let (Some(subagent_id), Some(tool_call)) = (
                        obj.get("subagent_id").and_then(|v| v.as_str()),
                        obj.get("tool_call").and_then(|v| v.as_object()),
                    ) {
                        let tool_name = tool_call
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let arguments = tool_call
                            .get("arguments")
                            .and_then(|v| v.as_object())
                            .cloned();

                        if interactive {
                            let _ = progress_bars.hide();
                        }
                        if is_stream_json_mode {
                            emit_stream_event(&StreamEvent::Notification {
                                extension_id: extension_id.to_string(),
                                data: NotificationData::Log {
                                    message: output::format_subagent_tool_call_message(
                                        subagent_id,
                                        tool_name,
                                    ),
                                },
                            });
                            return;
                        }
                        if !is_json_mode {
                            output::render_subagent_tool_call(
                                subagent_id,
                                tool_name,
                                arguments.as_ref(),
                                debug,
                            );
                            return;
                        }
                    }
                }
            }

            let (formatted, subagent_id, notif_type) =
                format_logging_notification(&log_notif.params.data, debug);

            if is_stream_json_mode {
                emit_stream_event(&StreamEvent::Notification {
                    extension_id: extension_id.to_string(),
                    data: NotificationData::Log {
                        message: formatted.clone(),
                    },
                });
            } else {
                display_log_notification(
                    &formatted,
                    subagent_id.as_deref(),
                    notif_type.as_deref(),
                    progress_bars,
                    interactive,
                    is_json_mode,
                );
            }
        }
        ServerNotification::ProgressNotification(prog_notif) => {
            if is_stream_json_mode {
                emit_stream_event(&StreamEvent::Notification {
                    extension_id: extension_id.to_string(),
                    data: NotificationData::Progress {
                        progress: prog_notif.params.progress,
                        total: prog_notif.params.total,
                        message: prog_notif.params.message.clone(),
                    },
                });
            } else {
                progress_bars.update(
                    &prog_notif.params.progress_token.0.to_string(),
                    prog_notif.params.progress,
                    prog_notif.params.total,
                    prog_notif.params.message.as_deref(),
                );
            }
        }
        ServerNotification::CustomNotification(notification) => {
            if let Some(params) = parse_shell_output_notification(notification) {
                if is_stream_json_mode
                    || is_json_mode
                    || !interactive
                    || !std::io::stdout().is_terminal()
                {
                    return;
                }
                display_shell_output_notification(params, progress_bars);
            }
        }
        _ => (),
    }
}

fn display_shell_output_notification(
    params: ShellOutputNotificationParams,
    progress_bars: &mut output::McpSpinners,
) {
    if params.truncated {
        return;
    }

    let max_width = console::Term::stdout()
        .size_checked()
        .map(|(_, width)| usize::from(width).saturating_sub(SHELL_STATUS_RESERVED_WIDTH))
        .unwrap_or(SHELL_STATUS_FALLBACK_WIDTH);
    let lines = latest_shell_output_lines(&params, max_width)
        .into_iter()
        .map(|(stream, line)| match stream {
            ShellOutputStream::Stdout => console::style(line).dim().to_string(),
            ShellOutputStream::Stderr => console::style(line).yellow().dim().to_string(),
        })
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        progress_bars.log_shell_output(lines, SHELL_STATUS_MAX_LINES);
    }
}

fn latest_shell_output_lines(
    params: &ShellOutputNotificationParams,
    max_width: usize,
) -> Vec<(ShellOutputStream, String)> {
    let mut lines = params
        .chunks
        .iter()
        .rev()
        .flat_map(|chunk| {
            chunk
                .output
                .lines()
                .rev()
                .map(move |line| (chunk.stream, line))
        })
        .take(SHELL_STATUS_MAX_LINES)
        .map(|(stream, line)| {
            let line = output::sanitize_terminal_line(line);
            (stream, safe_truncate(&line, max_width))
        })
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

/// Format a logging notification from MCP, returns (formatted_message, subagent_id, notification_type)
fn format_logging_notification(
    data: &Value,
    debug: bool,
) -> (String, Option<String>, Option<String>) {
    match data {
        Value::String(s) => (s.clone(), None, None),
        Value::Object(o) => {
            if let Some(Value::String(msg)) = o.get("message") {
                let subagent_id = o.get("subagent_id").and_then(|v| v.as_str());
                let notification_type = o.get("type").and_then(|v| v.as_str());

                let formatted = match notification_type {
                    Some("subagent_created") | Some("completed") | Some("terminated") => {
                        format!("🤖 {}", msg)
                    }
                    Some("tool_usage") | Some("tool_completed") | Some("tool_error") => {
                        format!("🔧 {}", msg)
                    }
                    Some("message_processing") | Some("turn_progress") => {
                        format!("💭 {}", msg)
                    }
                    Some("response_generated") => {
                        let config = Config::global();
                        let min_priority = config
                            .get_param::<f32>("GOOSE_CLI_MIN_PRIORITY")
                            .ok()
                            .unwrap_or(output::DEFAULT_MIN_PRIORITY);

                        if min_priority > 0.1 && !debug {
                            if let Some(response_content) = msg.strip_prefix("Responded: ") {
                                format!("🤖 Responded: {}", safe_truncate(response_content, 100))
                            } else {
                                format!("🤖 {}", msg)
                            }
                        } else {
                            format!("🤖 {}", msg)
                        }
                    }
                    _ => msg.to_string(),
                };
                (
                    formatted,
                    subagent_id.map(str::to_string),
                    notification_type.map(str::to_string),
                )
            } else if let Some(Value::String(output)) = o.get("output") {
                let notification_type = o.get("type").and_then(|v| v.as_str()).map(str::to_string);
                (output.to_owned(), None, notification_type)
            } else if let Some(result) = format_task_execution_notification(data) {
                result
            } else {
                (data.to_string(), None, None)
            }
        }
        v => (v.to_string(), None, None),
    }
}

/// Display a logging notification based on its type and context
fn display_log_notification(
    formatted_message: &str,
    subagent_id: Option<&str>,
    notification_type: Option<&str>,
    progress_bars: &mut output::McpSpinners,
    interactive: bool,
    is_json_mode: bool,
) {
    if subagent_id.is_some() {
        if interactive {
            let _ = progress_bars.hide();
            if !is_json_mode {
                println!("{}", console::style(formatted_message).green().dim());
            }
        } else if !is_json_mode {
            progress_bars.log(formatted_message);
        }
    } else if let Some(ntype) = notification_type {
        if ntype == TASK_EXECUTION_NOTIFICATION_TYPE {
            if interactive {
                let _ = progress_bars.hide();
            }
            if !is_json_mode {
                for line in formatted_message.lines() {
                    println!("    {}", console::style(line).dim());
                }
                std::io::stdout().flush().unwrap();
            }
        } else if ntype == "shell_output" {
            let config = Config::global();
            let min_priority = config
                .get_param::<f32>("GOOSE_CLI_MIN_PRIORITY")
                .ok()
                .unwrap_or(output::DEFAULT_MIN_PRIORITY);

            if min_priority < 0.1 {
                if interactive {
                    let _ = progress_bars.hide();
                }
                if !is_json_mode {
                    println!("    {}", console::style(formatted_message).dim());
                }
            }
        }
    } else if output::is_showing_thinking() {
        output::set_thinking_message(&formatted_message.to_string());
    } else {
        progress_bars.log(formatted_message);
    }
}

/// Log tool request/response metrics
fn log_tool_metrics(message: &Message, messages: &Conversation) {
    for content in &message.content {
        if let MessageContent::ToolRequest(tool_request) = content {
            if let Ok(tool_call) = &tool_request.tool_call {
                tracing::info!(
                    monotonic_counter.goose.tool_calls = 1,
                    tool_name = %tool_call.name,
                    "Tool call started"
                );
            }
        }
        if let MessageContent::ToolResponse(tool_response) = content {
            let tool_name = messages
                .iter()
                .rev()
                .find_map(|msg| {
                    msg.content.iter().find_map(|c| {
                        if let MessageContent::ToolRequest(req) = c {
                            if req.id == tool_response.id {
                                req.tool_call.as_ref().ok().map(|tc| tc.name.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_else(|| "unknown".to_string().into());

            let result_status = if tool_response.tool_result.is_ok() {
                "success"
            } else {
                "error"
            };
            tracing::info!(
                monotonic_counter.goose.tool_completions = 1,
                tool_name = %tool_name,
                result = %result_status,
                "Tool call completed"
            );
        }
    }
}

/// Handle and display an agent error
fn handle_agent_error(e: &anyhow::Error, is_stream_json_mode: bool) {
    let error_msg = e.to_string();

    if is_stream_json_mode {
        emit_stream_event(&StreamEvent::Error {
            error: error_msg.clone(),
        });
    }

    if e.downcast_ref::<goose_providers::errors::ProviderError>()
        .map(|provider_error| {
            matches!(
                provider_error,
                goose_providers::errors::ProviderError::ContextLengthExceeded(_)
            )
        })
        .unwrap_or(false)
    {
        if !is_stream_json_mode {
            output::render_text(
                "Compaction requested. Should have happened in the agent!",
                Some(Color::Yellow),
                true,
            );
        }
        warn!("Compaction requested. Should have happened in the agent!");
    }

    if !is_stream_json_mode {
        eprintln!("Error: {}", error_msg);
    }
}

fn headless_run_error(
    interactive: bool,
    cancelled: bool,
    stream_error: Option<anyhow::Error>,
    messages: &Conversation,
) -> Option<anyhow::Error> {
    if interactive {
        return None;
    }

    stream_error
        .or_else(|| cancelled.then(|| anyhow::anyhow!("Headless run interrupted")))
        .or_else(|| {
            messages
                .last()
                .and_then(|message| message.content.iter().find_map(MessageContent::as_error))
                .map(|error| anyhow::anyhow!(error.message.clone()))
        })
}

async fn get_reasoner(
) -> Result<(Arc<dyn Provider>, goose_providers::model::ModelConfig), anyhow::Error> {
    use goose::providers::create;

    let config = Config::global();

    // Try planner-specific provider first, fall back to default provider
    let provider = if let Ok(provider) = config.get_param::<String>("GOOSE_PLANNER_PROVIDER") {
        provider
    } else {
        println!("WARNING: GOOSE_PLANNER_PROVIDER not found. Using default provider...");
        config
            .get_goose_provider()
            .expect("No provider configured. Run 'goose configure' first")
    };

    // Try planner-specific model first, fall back to default model
    let model = if let Ok(model) = config.get_param::<String>("GOOSE_PLANNER_MODEL") {
        model
    } else {
        println!("WARNING: GOOSE_PLANNER_MODEL not found. Using default model...");
        config
            .get_goose_model()
            .expect("No model configured. Run 'goose configure' first")
    };

    let model_config =
        goose::model_config::model_config_from_user_config(&provider, model.as_str())?;
    let extensions = goose::config::extensions::get_enabled_extensions_with_config(config);
    let reasoner = create(&provider, extensions).await?;

    Ok((reasoner, model_config))
}

/// Format elapsed time duration
/// Shows seconds if less than 60, otherwise shows minutes:seconds
fn format_elapsed_time(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    if total_secs < 60 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        let minutes = total_secs / 60;
        let seconds = total_secs % 60;
        format!("{}m {:02}s", minutes, seconds)
    }
}

fn build_switched_model_config(
    provider_name: &str,
    model_name: &str,
    current_model_config: &goose_providers::model::ModelConfig,
) -> Result<goose_providers::model::ModelConfig> {
    goose::model_config::model_config_from_user_config(provider_name, model_name)
        .map(|config| {
            config
                .with_temperature(current_model_config.temperature)
                .with_toolshim(current_model_config.toolshim)
                .with_toolshim_model(current_model_config.toolshim_model.clone())
        })
        .map_err(|e| anyhow::anyhow!("Failed to create model configuration: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use goose::agents::extension::Envs;
    use goose::config::ExtensionConfig;
    use goose::conversation::message::MessageErrorKind;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Duration;
    use test_case::test_case;

    #[test]
    fn only_headless_terminal_failures_are_propagated() {
        let messages = Conversation::new_unvalidated([
            Message::assistant().with_error(MessageErrorKind::Other, "provider failed")
        ]);

        assert_eq!(
            headless_run_error(
                false,
                false,
                Some(anyhow::anyhow!("stream failed")),
                &Conversation::default(),
            )
            .unwrap()
            .to_string(),
            "stream failed"
        );
        assert_eq!(
            headless_run_error(false, false, None, &messages)
                .unwrap()
                .to_string(),
            "provider failed"
        );
        assert_eq!(
            headless_run_error(false, true, None, &Conversation::default())
                .unwrap()
                .to_string(),
            "Headless run interrupted"
        );
        assert!(headless_run_error(
            true,
            true,
            Some(anyhow::anyhow!("stream failed")),
            &messages,
        )
        .is_none());
    }

    #[test]
    fn provider_only_confirmation_preserves_authoritative_request() {
        let arguments = json!({"command": "cat ~/.ssh/id_rsa"})
            .as_object()
            .unwrap()
            .clone();
        let message = Message::assistant().with_action_required(
            "provider-request",
            "Bash".to_string(),
            arguments.clone(),
            Some("Review this request".to_string()),
        );

        let request = find_tool_confirmation(&message).unwrap();

        assert_eq!(request.id, "provider-request");
        assert_eq!(request.tool_name, "Bash");
        assert_eq!(request.arguments, arguments);
        assert_eq!(request.prompt.as_deref(), Some("Review this request"));
    }

    #[test]
    fn planner_classification_excludes_user_only_content() {
        use rmcp::model::{Annotations, Role, TextContent};

        let user_only = TextContent::new("user-only plan")
            .with_annotations(Annotations::default().with_audience(vec![Role::User]));
        let assistant_only = TextContent::new("agent classification text")
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant]));
        let mixed = Message::assistant()
            .with_content(MessageContent::Text(user_only.clone()))
            .with_content(MessageContent::Text(assistant_only));

        assert_eq!(
            planner_classification_text(&mixed).unwrap(),
            "agent classification text"
        );
        assert!(planner_classification_text(
            &Message::assistant().with_content(MessageContent::Text(user_only))
        )
        .is_err());
    }

    #[test]
    fn planner_history_is_fixed_after_audience_projection() {
        use rmcp::model::{Annotations, Role, TextContent};

        let hidden_separator = MessageContent::Text(
            TextContent::new("hidden separator")
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let history = Conversation::new_unvalidated([
            Message::user().with_text("first request"),
            Message::assistant().with_content(hidden_separator),
            Message::user().with_text("second request"),
        ]);

        let provider_messages = planner_provider_messages(&history).agent_visible_messages();

        assert_eq!(provider_messages.len(), 1);
        assert_eq!(provider_messages[0].role, Role::User);
        assert_eq!(
            provider_messages[0].as_concat_text(),
            "first request\nsecond request"
        );
        assert!(!provider_messages[0]
            .as_concat_text()
            .contains("hidden separator"));
    }

    #[test]
    fn planner_history_excludes_turn_context_events() {
        use goose::conversation::message::MessageMetadata;

        let history = Conversation::new_unvalidated([
            Message::user().with_text("plan the refactor"),
            Message::user()
                .with_text("<turn-context>cwd /repo, todo: ship v2</turn-context>")
                .with_metadata(MessageMetadata::agent_only().with_turn_context()),
            Message::assistant().with_text("on it"),
        ]);

        let provider_text = planner_provider_messages(&history)
            .agent_visible_messages()
            .iter()
            .map(|message| message.as_concat_text())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(provider_text.contains("plan the refactor"));
        assert!(
            !provider_text.contains("turn-context"),
            "the planner prompt has no turn-context instructions, so blocks must not reach it"
        );
    }

    #[test]
    fn test_format_elapsed_time_under_60_seconds() {
        // Test sub-second duration
        let duration = Duration::from_millis(500);
        assert_eq!(format_elapsed_time(duration), "0.50s");

        // Test exactly 1 second
        let duration = Duration::from_secs(1);
        assert_eq!(format_elapsed_time(duration), "1.00s");

        // Test 45.75 seconds
        let duration = Duration::from_millis(45750);
        assert_eq!(format_elapsed_time(duration), "45.75s");

        // Test 59.99 seconds
        let duration = Duration::from_millis(59990);
        assert_eq!(format_elapsed_time(duration), "59.99s");
    }

    #[test]
    fn test_format_elapsed_time_minutes() {
        // Test exactly 60 seconds (1 minute)
        let duration = Duration::from_secs(60);
        assert_eq!(format_elapsed_time(duration), "1m 00s");

        // Test 61 seconds (1 minute 1 second)
        let duration = Duration::from_secs(61);
        assert_eq!(format_elapsed_time(duration), "1m 01s");

        // Test 90 seconds (1 minute 30 seconds)
        let duration = Duration::from_secs(90);
        assert_eq!(format_elapsed_time(duration), "1m 30s");

        // Test 119 seconds (1 minute 59 seconds)
        let duration = Duration::from_secs(119);
        assert_eq!(format_elapsed_time(duration), "1m 59s");

        // Test 120 seconds (2 minutes)
        let duration = Duration::from_secs(120);
        assert_eq!(format_elapsed_time(duration), "2m 00s");

        // Test 605 seconds (10 minutes 5 seconds)
        let duration = Duration::from_secs(605);
        assert_eq!(format_elapsed_time(duration), "10m 05s");

        // Test 3661 seconds (61 minutes 1 second)
        let duration = Duration::from_secs(3661);
        assert_eq!(format_elapsed_time(duration), "61m 01s");
    }

    #[test]
    fn test_format_elapsed_time_edge_cases() {
        // Test zero duration
        let duration = Duration::from_secs(0);
        assert_eq!(format_elapsed_time(duration), "0.00s");

        // Test very small duration (1 millisecond)
        let duration = Duration::from_millis(1);
        assert_eq!(format_elapsed_time(duration), "0.00s");

        // Test fractional seconds are truncated for minute display
        // 60.5 seconds should still show as 1m 00s (not 1m 00.5s)
        let duration = Duration::from_millis(60500);
        assert_eq!(format_elapsed_time(duration), "1m 00s");
    }

    #[test_case(
        "/usr/bin/my-server",
        ExtensionConfig::Stdio {
            name: "my-server".into(),
            cmd: "/usr/bin/my-server".into(),
            args: vec![],
            envs: Envs::default(),
            env_keys: vec![],
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(goose::config::DEFAULT_EXTENSION_TIMEOUT),
            cwd: None,
            bundled: None,
            available_tools: vec![],
        }
        ; "name_from_cmd_basename"
    )]
    #[test_case(
        "MY_SECRET=s3cret npx -y @modelcontextprotocol/server-everything",
        ExtensionConfig::Stdio {
            name: "npx".into(),
            cmd: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
            envs: Envs::new([("MY_SECRET".into(), "s3cret".into())].into()),
            env_keys: vec![],
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(goose::config::DEFAULT_EXTENSION_TIMEOUT),
            cwd: None,
            bundled: None,
            available_tools: vec![],
        }
        ; "env_prefix_name_from_cmd"
    )]
    #[test_case(
        r#""/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java" -classpath "/path/with spaces/lib.jar" Main"#,
        ExtensionConfig::Stdio {
            name: "java".into(),
            cmd: "/Applications/IntelliJ IDEA.app/Contents/jbr/Contents/Home/bin/java".into(),
            args: vec!["-classpath".into(), "/path/with spaces/lib.jar".into(), "Main".into()],
            envs: Envs::default(),
            env_keys: vec![],
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(goose::config::DEFAULT_EXTENSION_TIMEOUT),
            cwd: None,
            bundled: None,
            available_tools: vec![],
        }
        ; "quoted_path_with_spaces"
    )]
    fn test_parse_stdio_extension(input: &str, expected: ExtensionConfig) {
        assert_eq!(CliSession::parse_stdio_extension(input).unwrap(), expected);
    }

    #[test]
    fn test_parse_stdio_extension_no_command() {
        assert!(CliSession::parse_stdio_extension("").is_err());
    }

    fn stdio_name(input: &str) -> String {
        CliSession::parse_stdio_extension(input).unwrap().name()
    }

    #[test]
    fn test_parse_stdio_extension_explicit_name() {
        assert_eq!(stdio_name("word:python -m word_mcp"), "word");
        assert_eq!(stdio_name("Word-One:python -m word_mcp"), "word-one");
        let absolute = CliSession::parse_stdio_extension("memory:/usr/local/bin/mcp").unwrap();
        assert_eq!(absolute.name(), "memory");
        assert!(matches!(
            absolute,
            ExtensionConfig::Stdio { cmd, .. } if cmd == "/usr/local/bin/mcp"
        ));
        let config = CliSession::parse_stdio_extension("memory:API_KEY=k npx -y srv").unwrap();
        let ExtensionConfig::Stdio {
            name,
            cmd,
            args,
            envs,
            ..
        } = config
        else {
            panic!("expected a stdio extension");
        };
        assert_eq!(name, "memory");
        assert_eq!(cmd, "npx");
        assert_eq!(args, vec!["-y".to_string(), "srv".to_string()]);
        assert_eq!(envs.get_env().get("API_KEY").map(String::as_str), Some("k"));
    }

    #[test_case("C:\\Program Files\\srv.exe --stdio" ; "windows_drive_letter")]
    #[test_case("srv://not-a-name" ; "url_like_command")]
    #[test_case("npx -y pkg:latest" ; "colon_after_a_space")]
    #[test_case("word:" ; "empty_command")]
    fn test_split_extension_name_prefix_leaves_command_alone(input: &str) {
        let (name, rest) = split_extension_name_prefix(input);
        assert_eq!(name, None);
        assert_eq!(rest, input);
    }

    #[test]
    fn test_derive_extension_name_from_command() {
        assert_eq!(
            derive_extension_name_from_command("python", &["-m".into(), "word_mcp".into()]),
            "python_m_word_mcp"
        );
        assert_eq!(
            derive_extension_name_from_command(
                "npx",
                &[
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into()
                ],
            ),
            "server-filesystem"
        );
        assert_eq!(
            derive_extension_name_from_command("/usr/local/bin/srv", &[]),
            "srv"
        );
    }

    #[test]
    fn test_build_switched_model_config_rebuilds_target_model_settings() {
        let _guard = env_lock::lock_env([
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_TEMPERATURE", None::<&str>),
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_TOOLSHIM", None::<&str>),
            ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
        ]);

        let current_model_config = goose_providers::model::ModelConfig {
            model_name: "gpt-4o".to_string(),
            context_limit: Some(128_000),
            temperature: Some(0.25),
            max_tokens: Some(16_384),
            toolshim: true,
            toolshim_model: Some("qwen2.5-coder".to_string()),
            request_params: Some(HashMap::from([(
                "anthropic_beta".to_string(),
                serde_json::json!(["output-128k-2025-02-19"]),
            )])),
            reasoning: Some(false),
            supports_vision: Some(true),
            request_headers: None,
        };

        let switched =
            build_switched_model_config("openai", "gpt-5.4", &current_model_config).unwrap();
        let expected = goose_providers::model::ModelConfig::new("gpt-5.4")
            .with_canonical_limits("openai")
            .with_temperature(Some(0.25))
            .with_toolshim(true)
            .with_toolshim_model(Some("qwen2.5-coder".to_string()));

        assert_eq!(switched.model_name, expected.model_name);
        assert_eq!(switched.context_limit, expected.context_limit);
        assert_eq!(switched.max_tokens, expected.max_tokens);
        assert_eq!(switched.request_params, expected.request_params);
        assert_eq!(switched.reasoning, expected.reasoning);
        assert_eq!(switched.temperature, Some(0.25));
        assert!(switched.toolshim);
        assert_eq!(switched.toolshim_model.as_deref(), Some("qwen2.5-coder"));
    }

    #[test]
    fn test_build_switched_model_config_detects_effort_suffix_change() {
        let _guard = env_lock::lock_env([
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_TEMPERATURE", None::<&str>),
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_TOOLSHIM", None::<&str>),
            ("GOOSE_TOOLSHIM_OLLAMA_MODEL", None::<&str>),
            ("GOOSE_THINKING_EFFORT", None::<&str>),
        ]);

        let current = goose_providers::model::ModelConfig::new("gpt-5.4-high")
            .with_canonical_limits("openai");
        assert_eq!(current.model_name, "gpt-5.4");
        assert_eq!(
            current.thinking_effort(),
            Some(goose_providers::thinking::ThinkingEffort::High)
        );

        let switched = build_switched_model_config("openai", "gpt-5.4", &current).unwrap();

        assert_eq!(switched.model_name, current.model_name);
        assert_ne!(switched.thinking_effort(), current.thinking_effort());
    }

    #[test]
    fn test_split_command_args_windows_paths() {
        assert_eq!(
            goose::utils::split_command_args(r"C:\tools\mcp.exe --arg value").unwrap(),
            vec![r"C:\tools\mcp.exe", "--arg", "value"]
        );
        assert_eq!(
            goose::utils::split_command_args(r#""C:\Program Files\server\mcp.exe" --arg"#).unwrap(),
            vec![r"C:\Program Files\server\mcp.exe", "--arg"]
        );
    }

    #[test]
    fn test_split_command_args_unmatched_quote() {
        assert!(goose::utils::split_command_args(r#""unmatched"#).is_err());
    }

    #[test_case(
        "https://mcp.kiwi.com", 300,
        ExtensionConfig::StreamableHttp {
            name: "mcp_kiwi_com".into(),
            uri: "https://mcp.kiwi.com".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::new(),
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(300),
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: vec![],
        }
        ; "name_from_host"
    )]
    #[test_case(
        "http://localhost:8080/api", 300,
        ExtensionConfig::StreamableHttp {
            name: "localhost_8080_api".into(),
            uri: "http://localhost:8080/api".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::new(),
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(300),
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: vec![],
        }
        ; "port_and_path"
    )]
    #[test_case(
        "http://localhost:9090/other", 300,
        ExtensionConfig::StreamableHttp {
            name: "localhost_9090_other".into(),
            uri: "http://localhost:9090/other".into(),
            envs: Envs::default(),
            env_keys: vec![],
            headers: HashMap::new(),
            description: goose::config::DEFAULT_EXTENSION_DESCRIPTION.to_string(),
            timeout: Some(300),
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: vec![],
        }
        ; "different_port_and_path"
    )]
    fn test_parse_streamable_http_extension(url: &str, timeout: u64, expected: ExtensionConfig) {
        assert_eq!(
            CliSession::parse_streamable_http_extension(url, timeout),
            expected
        );
    }

    #[tokio::test]
    async fn new_session_inherits_provider_model_and_working_dir() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let old = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "CLI Session".to_string(),
                goose::session::SessionType::User,
                GooseMode::Auto,
            )
            .await
            .unwrap();

        sm.update(&old.id)
            .provider_name("anthropic")
            .model_config(goose_providers::model::ModelConfig::new("test-model"))
            .accumulated_usage(goose_providers::conversation::token_usage::Usage::new(
                Some(100),
                Some(50),
                Some(150),
            ))
            .apply()
            .await
            .unwrap();

        sm.add_message(&old.id, &Message::user().with_text("hello"))
            .await
            .unwrap();

        let mut extension_data = goose::session::ExtensionData::new();
        extension_data.set_extension_state("test", "v0", serde_json::json!("marker"));
        sm.update(&old.id)
            .extension_data(extension_data)
            .apply()
            .await
            .unwrap();

        let old = sm.get_session(&old.id, false).await.unwrap();

        let new_id = create_successor_session(&sm, &old, GooseMode::Chat)
            .await
            .unwrap();

        assert_ne!(new_id, old.id);

        let new_session = sm.get_session(&new_id, true).await.unwrap();
        assert_eq!(new_session.provider_name, old.provider_name);
        assert_eq!(
            new_session.model_config.as_ref().map(|m| &m.model_name),
            old.model_config.as_ref().map(|m| &m.model_name)
        );
        assert_eq!(new_session.goose_mode, GooseMode::Chat);
        assert_eq!(new_session.working_dir, old.working_dir);
        assert_eq!(new_session.session_type, old.session_type);
        assert!(new_session.conversation.unwrap().messages().is_empty());
        assert_eq!(new_session.usage.total_tokens, None);
        assert_eq!(old.accumulated_usage.total_tokens, Some(150));
        assert_eq!(new_session.accumulated_usage.total_tokens, None);

        let reloaded_old = sm.get_session(&old.id, true).await.unwrap();
        let old_messages = reloaded_old.conversation.unwrap().messages().to_vec();
        assert_eq!(old_messages.len(), 1);
        assert_eq!(old_messages[0].as_concat_text(), "hello");
        assert_eq!(
            reloaded_old
                .extension_data
                .get_extension_state("test", "v0"),
            Some(&serde_json::json!("marker"))
        );
    }

    struct StubProvider;

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn get_name(&self) -> &str {
            "stub"
        }

        async fn stream(
            &self,
            _model_config: &goose_providers::model::ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[rmcp::model::Tool],
        ) -> std::result::Result<
            goose::providers::base::MessageStream,
            goose_providers::errors::ProviderError,
        > {
            Ok(goose::providers::base::stream_from_single_message(
                Message::assistant().with_text("stub reply"),
                ProviderUsage::new(
                    "stub".to_string(),
                    goose_providers::conversation::token_usage::Usage::default(),
                ),
            ))
        }
    }

    async fn session_with_loader(
        extension_loading: Option<AbortOnDropHandle<Vec<ExtensionFailure>>>,
        refresh_completions: bool,
    ) -> CliSession {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let session_manager = SessionManager::new(temp_dir.path().to_path_buf());
        let session = session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "Loading gate test".to_string(),
                goose::session::SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        let agent = goose::agents::Agent::with_config(goose::agents::AgentConfig::new(
            Arc::new(session_manager),
            Arc::new(goose::config::PermissionManager::new(
                temp_dir.path().to_path_buf(),
            )),
            None,
            GooseMode::default(),
            // Disable background session naming so the test agent starts no
            // provider-dependent tasks.
            true,
            goose::agents::GoosePlatform::GooseCli,
        ));
        agent
            .update_provider(
                Arc::new(StubProvider),
                goose_providers::model::ModelConfig::new("stub-model"),
                &session.id,
            )
            .await
            .unwrap();

        CliSession::new(
            Arc::new(agent),
            session.id,
            false,
            None,
            None,
            None,
            None,
            "text".to_string(),
            false,
            refresh_completions,
            extension_loading,
        )
        .await
    }

    #[tokio::test]
    async fn commands_wait_for_background_extension_loading() {
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let loader = AbortOnDropHandle::new(tokio::spawn(async move {
            let _ = released.await;
            Vec::<ExtensionFailure>::new()
        }));

        let mut session = session_with_loader(Some(loader), false).await;
        let mut editor = session.create_editor().unwrap();

        let handled = tokio::spawn(async move {
            let history = HistoryManager::new();
            session
                .handle_input(
                    InputResult::Plan(input::PlanCommandOptions {
                        message_text: String::new(),
                    }),
                    &history,
                    &mut editor,
                    &[],
                )
                .await
                .expect("handle_input failed");
            session.run_mode
        });

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !handled.is_finished(),
            "a command was handled before background extension loading finished"
        );

        release.send(()).unwrap();
        let run_mode = handled.await.unwrap();
        assert!(matches!(run_mode, RunMode::Plan));
    }

    #[tokio::test]
    async fn ensure_extensions_loaded_drains_the_loader_once() {
        let loader = AbortOnDropHandle::new(tokio::spawn(async { Vec::<ExtensionFailure>::new() }));
        let mut session = session_with_loader(Some(loader), false).await;

        session.ensure_extensions_loaded(false).await.unwrap();
        assert!(session.extension_loading.is_none());

        // A second pass is a no-op, not an error.
        session.ensure_extensions_loaded(false).await.unwrap();
        assert!(session.extension_loading.is_none());
    }

    #[tokio::test]
    async fn background_loader_refreshes_completions_before_the_gate_runs() {
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let loader = AbortOnDropHandle::new(tokio::spawn(async move {
            let _ = released.await;
            Vec::<ExtensionFailure>::new()
        }));
        let session = session_with_loader(Some(loader), true).await;

        assert!(session
            .completion_cache
            .read()
            .unwrap()
            .current_session_provider
            .is_empty());
        release.send(()).unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if session
                    .completion_cache
                    .read()
                    .unwrap()
                    .current_session_provider
                    == "stub"
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion cache was not refreshed in the background");
        assert!(session.extension_loading.is_some());
    }

    #[tokio::test]
    async fn headless_loader_skips_completion_refresh() {
        let loader = AbortOnDropHandle::new(tokio::spawn(async { Vec::<ExtensionFailure>::new() }));
        let mut session = session_with_loader(Some(loader), false).await;

        session.ensure_extensions_loaded(false).await.unwrap();

        assert!(session
            .completion_cache
            .read()
            .unwrap()
            .current_session_provider
            .is_empty());
    }
}
