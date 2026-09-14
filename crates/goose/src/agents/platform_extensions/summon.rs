use crate::agents::extension::PlatformExtensionContext;
use crate::agents::mcp_client::{Error, McpClientTrait};
use crate::agents::subagent_handler::{run_subagent_task, OnMessageCallback, SubagentRunParams};
use crate::agents::subagent_task_config::{TaskConfig, DEFAULT_SUBAGENT_MAX_TURNS};
use crate::agents::tool_execution::{ToolCallContext, ToolCallNotificationEmitter};
use crate::agents::AgentConfig;
use crate::config::paths::Paths;
use crate::config::{Config, GooseMode};
use crate::providers;
use crate::recipe::build_recipe::build_recipe_from_template;
use crate::recipe::local_recipes::load_local_recipe_file;
use crate::recipe::{Recipe, RecipeParameter, Settings, RECIPE_FILE_EXTENSIONS};
use crate::session::extension_data::EnabledExtensionsState;
use crate::session::SessionType;
use crate::sources::parse_frontmatter;
use crate::utils::safe_truncate;
use anyhow::Result;
use async_trait::async_trait;
use goose_agent::operation::messages_since_kickoff;
use goose_sdk_types::custom_requests::{SourceEntry, SourceType};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeResult, JsonObject, ListToolsResult,
    MetaObject, Role, ServerCapabilities, ServerNotification, Tool,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub static EXTENSION_NAME: &str = "summon";

const SUBAGENT_DESCRIPTION_BUDGET: usize = 160;

const TASK_LABEL_BUDGET: usize = 60;

fn durable_assistant_turn_count(conversation: &crate::conversation::Conversation) -> u32 {
    let Ok(messages) = messages_since_kickoff(conversation) else {
        return 0;
    };
    let mut turns = 0;
    let mut in_assistant_block = false;
    for message in messages.iter().rev() {
        // Compaction summaries and continuation prompts are assistant-role
        // scaffolding, but are not user-visible task turns. Ignore them
        // without merging across a hidden user replay below.
        if message.role == Role::Assistant && !message.is_user_visible() {
            continue;
        }
        if message.role == Role::Assistant {
            if !in_assistant_block {
                turns += 1;
                in_assistant_block = true;
            }
        } else {
            in_assistant_block = false;
        }
    }
    turns
}

fn kind_plural(kind: SourceType) -> &'static str {
    match kind {
        SourceType::Subrecipe => "Subrecipes",
        SourceType::Recipe => "Recipes",
        SourceType::Agent => "Agents",
        _ => "Other",
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct DelegateParams {
    pub instructions: Option<String>,
    pub source: Option<String>,
    pub parameters: Option<HashMap<String, serde_json::Value>>,
    pub extensions: Option<Vec<String>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_turns: Option<usize>,
    pub context: Option<String>,
    pub working_dir: Option<String>,
    #[serde(default)]
    pub r#async: bool,
}

pub struct BackgroundTask {
    pub id: String,
    pub description: String,
    pub started_at: Instant,
    pub turns: Arc<AtomicU32>,
    pub last_activity: Arc<AtomicU64>,
    pub handle: JoinHandle<Result<String>>,
    pub cancellation_token: CancellationToken,
    completion_token: CancellationToken,
    notification_sink: SharedNotificationSink,
}

fn spawn_background_task<F>(future: F) -> (JoinHandle<Result<String>>, CancellationToken)
where
    F: Future<Output = Result<String>> + Send + 'static,
{
    let completion_token = CancellationToken::new();
    let completion_guard = completion_token.clone().drop_guard();
    let handle = tokio::spawn(async move {
        let _completion_guard = completion_guard;
        future.await
    });
    (handle, completion_token)
}

pub struct CompletedTask {
    pub id: String,
    pub description: String,
    pub result: Result<String, String>,
    pub turns_taken: u32,
    pub duration: Duration,
    pub completed_at: Instant,
    notification_sink: SharedNotificationSink,
}

enum NotificationSink {
    Buffer(Vec<ServerNotification>),
    Emitter(ToolCallNotificationEmitter),
}

type SharedNotificationSink = Arc<Mutex<NotificationSink>>;

async fn yield_to_outer_tool_stream() {
    // The outer select may have polled its receiver before this future queues a
    // notification. Keep the result pending for the following select pass so
    // the now-ready receiver is observed before the terminal result.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

impl NotificationSink {
    fn route(&mut self, notification: ServerNotification) {
        match self {
            Self::Buffer(buffer) => buffer.push(notification),
            Self::Emitter(emitter) => emitter.emit_best_effort(notification),
        }
    }

    async fn attach(&mut self, emitter: Option<ToolCallNotificationEmitter>) {
        let Some(emitter) = emitter else {
            return;
        };
        while let Self::Buffer(buffered) = self {
            let Some(notification) = buffered.first().cloned() else {
                break;
            };
            emitter.emit_best_effort(notification);
            yield_to_outer_tool_stream().await;
            buffered.remove(0);
        }
        *self = Self::Emitter(emitter);
    }

    fn detach(&mut self) {
        if matches!(self, Self::Emitter(_)) {
            *self = Self::Buffer(Vec::new());
        }
    }

    fn buffered_len(&self) -> usize {
        match self {
            Self::Buffer(buffer) => buffer.len(),
            Self::Emitter(_) => 0,
        }
    }
}

fn merge_subrecipe_parameters(
    fixed_values: Option<&HashMap<String, String>>,
    provided_parameters: Option<&HashMap<String, serde_json::Value>>,
) -> HashMap<String, String> {
    let mut merged = fixed_values.cloned().unwrap_or_default();
    if let Some(provided_parameters) = provided_parameters {
        for (key, value) in provided_parameters {
            let value = match value {
                serde_json::Value::String(value) => value.clone(),
                other => other.to_string(),
            };
            merged.entry(key.clone()).or_insert(value);
        }
    }
    merged
}

/// Result from handle_load_task_result with structured metadata for the caller
#[derive(Debug)]
struct TaskLoadResult {
    content: Vec<ContentBlock>,
    status: &'static str,
    turns: Option<u32>,
    duration_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AgentMetadata {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

fn parse_agent_content(content: &str, path: &Path) -> Option<SourceEntry> {
    let (metadata, body): (AgentMetadata, String) = match parse_frontmatter(content) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => return None,
        Err(e) => {
            // Missing fields means this file has valid YAML but isn't an agent — skip silently.
            // Only warn on actual YAML syntax errors.
            if e.to_string().contains("missing field") {
                return None;
            }
            warn!("Failed to parse agent file {}: {}", path.display(), e);
            return None;
        }
    };

    let description = metadata.description.unwrap_or_else(|| {
        let model_info = metadata
            .model
            .as_ref()
            .map(|m| format!(" ({})", m))
            .unwrap_or_default();
        format!("Agent{}", model_info)
    });

    let mut properties = std::collections::HashMap::new();
    if let Some(model) = metadata.model {
        properties.insert("model".to_string(), serde_json::Value::String(model));
    }

    Some(SourceEntry {
        source_type: SourceType::Agent,
        name: metadata.name,
        description,
        content: body,
        path: path.to_string_lossy().into_owned(),
        global: false,
        writable: true,
        supporting_files: Vec::new(),
        properties,
    })
}

fn scan_recipes_from_dir(
    dir: &Path,
    kind: SourceType,
    suppress_config_warnings: bool,
    sources: &mut Vec<SourceEntry>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(source_dir) = dir.canonicalize() else {
        return;
    };
    let entries = match std::fs::read_dir(&source_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let path = source_dir.join(&file_name);

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !RECIPE_FILE_EXTENSIONS.contains(&ext) {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() || seen.contains(&name) {
            continue;
        }

        let content = match crate::skills::read_source_file(&source_dir, Path::new(&file_name)) {
            Ok(content) => content,
            Err(error) => {
                warn!("Failed to read recipe {}: {}", path.display(), error);
                continue;
            }
        };

        match Recipe::from_content(&content) {
            Ok(recipe) => {
                seen.insert(name.clone());
                sources.push(SourceEntry {
                    source_type: kind,
                    name,
                    description: recipe.description.clone(),
                    content: recipe.instructions.clone().unwrap_or_default(),
                    path: path.to_string_lossy().into_owned(),
                    global: false,
                    writable: true,
                    supporting_files: Vec::new(),
                    properties: std::collections::HashMap::new(),
                });
            }
            Err(e) => {
                // The working directory commonly contains project config like package.json
                // and tsconfig.json, which parse as valid JSON but lack Recipe fields. In that
                // case treat them as "not a recipe" rather than warning. Dedicated recipe
                // directories still warn so a real recipe with a typo is not silently dropped.
                if suppress_config_warnings && e.to_string().contains("missing field") {
                    continue;
                }
                warn!("Failed to parse recipe {}: {}", path.display(), e);
            }
        }
    }
}

fn scan_agents_from_dir(
    dir: &Path,
    sources: &mut Vec<SourceEntry>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(source_dir) = dir.canonicalize() else {
        return;
    };
    let entries = match std::fs::read_dir(&source_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let path = source_dir.join(&file_name);

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "md" {
            continue;
        }

        let content = match crate::skills::read_source_file(&source_dir, Path::new(&file_name)) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read agent file {}: {}", path.display(), e);
                continue;
            }
        };

        if let Some(source) = parse_agent_content(&content, &path) {
            if !seen.contains(&source.name) {
                seen.insert(source.name.clone());
                sources.push(source);
            }
        }
    }
}

pub fn discover_filesystem_sources(working_dir: &Path) -> Vec<SourceEntry> {
    let mut sources: Vec<SourceEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let home = dirs::home_dir();
    let config = Paths::config_dir();

    let local_recipe_dirs: Vec<PathBuf> = vec![
        working_dir.join(".goose/recipes"),
        working_dir.join(".agents/recipes"),
    ];

    let global_recipe_dirs: Vec<PathBuf> = std::env::var("GOOSE_RECIPE_PATH")
        .ok()
        .into_iter()
        .flat_map(|p| {
            let sep = if cfg!(windows) { ';' } else { ':' };
            p.split(sep).map(PathBuf::from).collect::<Vec<_>>()
        })
        .chain(
            [
                home.as_ref().map(|h| h.join(".goose/recipes")),
                Some(config.join("recipes")),
                home.as_ref().map(|h| h.join(".agents/recipes")),
            ]
            .into_iter()
            .flatten(),
        )
        .collect();

    let local_agent_dirs: Vec<PathBuf> = vec![
        working_dir.join(".goose/agents"),
        working_dir.join(".claude/agents"),
        working_dir.join(".agents/agents"),
    ];

    let global_agent_dirs: Vec<PathBuf> = [
        home.as_ref().map(|h| h.join(".goose/agents")),
        home.as_ref().map(|h| h.join(".agents/agents")),
        Some(config.join("agents")),
        home.as_ref().map(|h| h.join(".claude/agents")),
    ]
    .into_iter()
    .flatten()
    .collect();

    scan_recipes_from_dir(
        working_dir,
        SourceType::Recipe,
        true,
        &mut sources,
        &mut seen,
    );

    for dir in local_recipe_dirs {
        scan_recipes_from_dir(&dir, SourceType::Recipe, false, &mut sources, &mut seen);
    }

    for dir in local_agent_dirs {
        scan_agents_from_dir(&dir, &mut sources, &mut seen);
    }

    for dir in global_recipe_dirs {
        scan_recipes_from_dir(&dir, SourceType::Recipe, false, &mut sources, &mut seen);
    }

    for dir in global_agent_dirs {
        scan_agents_from_dir(&dir, &mut sources, &mut seen);
    }

    sources
}

fn build_instructions_with_context(context: &str, instructions: &str) -> String {
    let mut result = format!("# Reference Context\n\n{}", context);
    if !instructions.is_empty() {
        result.push_str(&format!("\n\n# Task Instructions\n\n{}", instructions));
    }
    result
}

fn build_subagent_instructions(session: Option<&crate::session::Session>) -> String {
    let Some(session) = session else {
        return String::new();
    };

    // filter the sources down to what we want even though currently that is what we get
    let mut sources: Vec<SourceEntry> = discover_filesystem_sources(&session.working_dir)
        .into_iter()
        .filter(|s| {
            matches!(
                s.source_type,
                SourceType::Agent | SourceType::Recipe | SourceType::Subrecipe
            )
        })
        .collect();

    // If the session is started from a recipe, also use the subrecipes for
    // that recipe as delegate targets
    if let Some(recipe) = session.recipe.as_ref() {
        if let Some(subs) = recipe.sub_recipes.as_ref() {
            let mut seen: std::collections::HashSet<String> =
                sources.iter().map(|s| s.name.clone()).collect();
            for sr in subs {
                if !seen.insert(sr.name.clone()) {
                    continue;
                }
                sources.push(SourceEntry {
                    source_type: SourceType::Subrecipe,
                    name: sr.name.clone(),
                    description: sr.description.clone().unwrap_or_default(),
                    content: String::new(),
                    path: sr.path.clone(),
                    global: false,
                    writable: false,
                    supporting_files: Vec::new(),
                    properties: std::collections::HashMap::new(),
                });
            }
        }
    }

    if sources.is_empty() {
        return String::new();
    }

    sources.sort_by(|a, b| (&a.source_type, &a.name).cmp(&(&b.source_type, &b.name)));
    let subagents: Vec<&SourceEntry> = sources.iter().collect();

    let names = subagents
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    out.push_str(
        "\n\nThe following named subagents are available in this session and \
         can be invoked through the `delegate` tool (run as a subagent) or \
         the `load` tool (read their instructions into your own context):\n",
    );

    let mut current_kind: Option<SourceType> = None;
    for s in &subagents {
        if current_kind != Some(s.source_type) {
            out.push_str(&format!("\n{}:", kind_plural(s.source_type)));
            current_kind = Some(s.source_type);
        }
        out.push_str(&format!(
            "\n• {} — {}",
            s.name,
            safe_truncate(&s.description, SUBAGENT_DESCRIPTION_BUDGET)
        ));
    }

    out.push_str(&format!(
        "\n\nWhen to call a subagent (one of [{names}]):\n\
         • `@<name>` in the user's message — always call that subagent.\n\
         • The user mentions a subagent by name without `@` — infer from \
         context whether they want it invoked, and if so, call it.\n\
         • The user's request strongly matches a subagent's description — \
         call it.\n\n\
         Calling a subagent normally means `delegate(source: \"<name>\", \
         instructions: ...)`, which runs it as an isolated subagent and \
         returns its result. Use `load(source: \"<name>\")` instead if you \
         only want to read the subagent's instructions into your own \
         context. For long-running work, pass `async: true` to `delegate` — \
         it returns a task id immediately, and you collect the result later \
         with `load(source: \"<task_id>\")`, which waits for completion.",
    ));

    out
}

fn round_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", (secs / 10) * 10)
    } else {
        format!("{}m", secs / 60)
    }
}

fn current_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Get maximum number of concurrent background tasks
fn max_background_tasks() -> usize {
    Config::global()
        .get_param::<usize>("GOOSE_MAX_BACKGROUND_TASKS")
        .unwrap_or(5)
}

fn completed_task_ttl() -> Duration {
    let secs = Config::global()
        .get_param::<u64>("GOOSE_COMPLETED_TASK_TTL_SECS")
        .unwrap_or(600);
    Duration::from_secs(secs)
}

fn is_session_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('_').collect();
    parts.len() == 2 && parts[0].len() == 8 && parts[0].chars().all(|c| c.is_ascii_digit())
}

pub struct SummonClient {
    info: InitializeResult,
    context: PlatformExtensionContext,
    source_cache: Mutex<Option<(Instant, PathBuf, Vec<SourceEntry>)>>,
    background_tasks: Mutex<HashMap<String, BackgroundTask>>,
    completed_tasks: Mutex<HashMap<String, CompletedTask>>,
}

impl Drop for SummonClient {
    fn drop(&mut self) {
        // Best-effort cancellation of running tasks on shutdown
        if let Ok(tasks) = self.background_tasks.try_lock() {
            for task in tasks.values() {
                task.cancellation_token.cancel();
            }
        }
    }
}

impl SummonClient {
    pub fn new(context: PlatformExtensionContext) -> Result<Self> {
        let info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(EXTENSION_NAME, "1.0.0").with_title("Summon"));

        Ok(Self {
            info,
            context,
            source_cache: Mutex::new(None),
            background_tasks: Mutex::new(HashMap::new()),
            completed_tasks: Mutex::new(HashMap::new()),
        })
    }

    async fn create_subagent_session(
        &self,
        task_config: &TaskConfig,
        name: String,
    ) -> Result<crate::session::Session, String> {
        let session = self
            .context
            .session_manager
            .create_session(
                task_config.parent_working_dir.clone(),
                name,
                SessionType::SubAgent,
                GooseMode::Auto,
            )
            .await
            .map_err(|e| format!("Failed to create subagent session: {}", e))?;

        if !task_config.parent_session_id.is_empty() {
            self.context
                .session_manager
                .update(&session.id)
                .parent_session_id(Some(task_config.parent_session_id.clone()))
                .apply()
                .await
                .map_err(|e| format!("Failed to link subagent to parent session: {}", e))?;
        }

        Ok(session)
    }

    fn notification_sink(emitter: Option<ToolCallNotificationEmitter>) -> SharedNotificationSink {
        Arc::new(Mutex::new(match emitter {
            Some(emitter) => NotificationSink::Emitter(emitter),
            None => NotificationSink::Buffer(Vec::new()),
        }))
    }

    async fn attach_notification_emitter(
        sink: &SharedNotificationSink,
        emitter: Option<ToolCallNotificationEmitter>,
    ) {
        sink.lock().await.attach(emitter).await;
    }

    async fn run_subagent_with_notifications<Run, RunFuture>(
        sink: SharedNotificationSink,
        run_subagent: Run,
    ) -> Result<String>
    where
        Run: FnOnce(tokio::sync::mpsc::UnboundedSender<ServerNotification>) -> RunFuture,
        RunFuture: Future<Output = Result<String>>,
    {
        let (notification_tx, mut notification_rx) = tokio::sync::mpsc::unbounded_channel();
        let run = run_subagent(notification_tx);
        tokio::pin!(run);

        loop {
            tokio::select! {
                biased;
                result = &mut run => {
                    while let Ok(notification) = notification_rx.try_recv() {
                        sink.lock().await.route(notification);
                        yield_to_outer_tool_stream().await;
                    }
                    yield_to_outer_tool_stream().await;
                    return result;
                }
                Some(notification) = notification_rx.recv() => {
                    sink.lock().await.route(notification);
                    yield_to_outer_tool_stream().await;
                }
            }
        }
    }

    fn create_load_tool(&self) -> Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Name of the source to load. If omitted, lists all available sources."
                },
                "cancel": {
                    "type": "boolean",
                    "default": false,
                    "description": "For running background tasks: cancel and return output."
                },
                "peek": {
                    "type": "boolean",
                    "default": false,
                    "description": "For running background tasks: check progress without blocking. Returns durable assistant-turn count, idle time, and recent tool activity."
                }
            }
        });

        Tool::new(
            "load",
            "Load knowledge into your current context or discover available sources.\n\n\
             Call with no arguments to list all available sources (subrecipes, recipes, agents).\n\
             Call with a source name to load its content into your context.\n\
             For background tasks: load(source: \"task_id\") waits for the task and returns the result.\n\
             To cancel a running task: load(source: \"task_id\", cancel: true) stops and returns output.\n\
             To check progress: load(source: \"task_id\", peek: true) returns status without blocking.\n\n\
             Examples:\n\
             - load() → Lists available sources\n\
             - load(source: \"deploy\") → Loads the deploy recipe\n\
             - load(source: \"20260219_1\") → Waits for background task, then returns result\n\
             - load(source: \"20260219_1\", peek: true) → Check task progress without waiting"
                .to_string(),
            schema.as_object().unwrap().clone(),
        )
    }

    fn create_delegate_tool(&self) -> Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "instructions": {
                    "type": "string",
                    "description": "Task instructions. Required for ad-hoc tasks."
                },
                "source": {
                    "type": "string",
                    "description": "Name of a recipe or agent to run."
                },
                "parameters": {
                    "type": "object",
                    "additionalProperties": true,
                    "description": "Parameters for the source (only valid with source)."
                },
                "extensions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Extensions to enable. Omit to inherit all, empty array for none."
                },
                "provider": {
                    "type": "string",
                    "description": "Override LLM provider."
                },
                "model": {
                    "type": "string",
                    "description": "Override model."
                },
                "temperature": {
                    "type": "number",
                    "description": "Override temperature."
                },
                "max_turns": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum turns for this delegate. Overrides recipe settings.max_turns and GOOSE_SUBAGENT_MAX_TURNS."
                },
                "context": {
                    "type": "string",
                    "description": "Reference context to inject into the delegate's system prompt. Use for background information, file contents, or constraints the delegate needs but that aren't part of the task instructions."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the delegate. Must be within the parent session's working directory. Defaults to the parent's working directory."
                },
                "async": {
                    "type": "boolean",
                    "default": false,
                    "description": "Run in background (default: false)."
                }
            }
        });

        Tool::new(
            "delegate",
            "Delegate a task to a subagent that runs independently with its own context.\n\n\
             Modes:\n\
             1. Ad-hoc: Provide `instructions` for a custom task\n\
             2. Source-based: Provide `source` name to run a subrecipe, recipe, or agent\n\
             3. Combined: Pair a source with a task (e.g., source: \"deploy\", instructions: \"deploy to staging\")\n\n\
             Effective Delegation:\n\
             - Delegates know only instructions + source content\n\
             - Delegates cannot coordinate. Same-file work = conflicts.\n\
             - Parallel: async: true, then load(taskId) to wait and get results. Single: sync.\n\n\
             Research (read-only): parallelize freely - delegates explore and report back.\n\
             Work (writes): partition files strictly - no two delegates touch the same file.\n\n\
             Decompose → async delegates → load(taskId) for each → synthesize."
                .to_string(),
            schema.as_object().unwrap().clone(),
        )
    }

    async fn get_working_dir(&self, session_id: &str) -> PathBuf {
        self.context
            .session_manager
            .get_session(session_id, false)
            .await
            .ok()
            .map(|s| s.working_dir)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    async fn get_sources(&self, session_id: &str, working_dir: &Path) -> Vec<SourceEntry> {
        let fs_sources = self.get_filesystem_sources(working_dir).await;

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut sources: Vec<SourceEntry> = Vec::new();

        self.add_subrecipes(session_id, &mut sources, &mut seen)
            .await;

        for source in fs_sources {
            if !seen.contains(&source.name) {
                seen.insert(source.name.clone());
                sources.push(source);
            }
        }

        sources.sort_by(|a, b| (&a.source_type, &a.name).cmp(&(&b.source_type, &b.name)));
        sources
    }

    async fn get_filesystem_sources(&self, working_dir: &Path) -> Vec<SourceEntry> {
        let mut cache = self.source_cache.lock().await;
        if let Some((cached_at, cached_dir, sources)) = cache.as_ref() {
            if cached_dir == working_dir && cached_at.elapsed() < Duration::from_secs(60) {
                return sources.clone();
            }
        }
        let sources = self.discover_filesystem_sources(working_dir);
        *cache = Some((Instant::now(), working_dir.to_path_buf(), sources.clone()));
        sources
    }

    async fn resolve_source(
        &self,
        session_id: &str,
        name: &str,
        working_dir: &Path,
    ) -> Result<Option<SourceEntry>, String> {
        let sources = self.get_sources(session_id, working_dir).await;

        Ok(sources.iter().find(|s| s.name == name).cloned())
    }

    async fn load_subrecipe_content(&self, session_id: &str, name: &str) -> Result<String, String> {
        let session = match self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
        {
            Ok(s) => s,
            Err(_) => return Ok(String::new()),
        };

        let sub_recipes = match session.recipe.as_ref().and_then(|r| r.sub_recipes.as_ref()) {
            Some(sr) => sr,
            None => return Ok(String::new()),
        };

        let sr = match sub_recipes.iter().find(|sr| sr.name == name) {
            Some(sr) => sr,
            None => return Ok(String::new()),
        };

        match load_local_recipe_file(&sr.path) {
            Ok(recipe_file) => Self::format_subrecipe_content(name, &recipe_file.content),
            Err(_) => Ok(String::new()),
        }
    }

    fn format_subrecipe_content(name: &str, raw_content: &str) -> Result<String, String> {
        let recipe = Recipe::from_content(raw_content)
            .map_err(|_| format!("Subrecipe '{}' is not a valid recipe", name))?;
        let mut content = recipe.instructions.unwrap_or_default();
        if let Some(params) = &recipe.parameters {
            if !params.is_empty() {
                content.push_str("\n\n");
                content.push_str(&Self::format_parameters(params));
            }
        }
        Ok(content)
    }

    fn discover_filesystem_sources(&self, working_dir: &Path) -> Vec<SourceEntry> {
        discover_filesystem_sources(working_dir)
    }

    async fn add_subrecipes(
        &self,
        session_id: &str,
        sources: &mut Vec<SourceEntry>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        let session = match self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
        {
            Ok(s) => s,
            Err(_) => return,
        };

        let sub_recipes = match session.recipe.as_ref().and_then(|r| r.sub_recipes.as_ref()) {
            Some(sr) => sr,
            None => return,
        };

        for sr in sub_recipes {
            if seen.contains(&sr.name) {
                continue;
            }
            seen.insert(sr.name.clone());

            let description = self.build_subrecipe_description(sr).await;

            sources.push(SourceEntry {
                source_type: SourceType::Subrecipe,
                name: sr.name.clone(),
                description,
                content: String::new(),
                path: sr.path.clone(),
                global: false,
                writable: true,
                supporting_files: Vec::new(),
                properties: std::collections::HashMap::new(),
            });
        }
    }

    async fn build_subrecipe_description(&self, sr: &crate::recipe::SubRecipe) -> String {
        if let Some(desc) = &sr.description {
            return desc.clone();
        }

        if let Ok(recipe_file) = load_local_recipe_file(&sr.path) {
            if let Ok(recipe) = Recipe::from_content(&recipe_file.content) {
                let mut desc = recipe.description.clone();

                if let Some(params) = &recipe.parameters {
                    if !params.is_empty() {
                        desc = format!("{}\n{}", desc, Self::format_parameters(params));
                    }
                }

                return desc;
            }
        }

        format!("Subrecipe from {}", sr.path)
    }

    fn format_parameters(params: &[RecipeParameter]) -> String {
        let mut out = String::from("Parameters:");
        for p in params {
            let mut detail = format!("\n  - {} ({}, {})", p.key, p.input_type, p.requirement);
            if let Some(default) = &p.default {
                detail.push_str(&format!(", default: \"{}\"", default));
            }
            if let Some(options) = &p.options {
                if !options.is_empty() {
                    detail.push_str(&format!(", options: [{}]", options.join(", ")));
                }
            }
            detail.push_str(&format!(": {}", p.description));
            out.push_str(&detail);
        }
        out
    }

    async fn handle_load(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
        notification_emitter: Option<ToolCallNotificationEmitter>,
    ) -> Result<CallToolResult, String> {
        self.cleanup_completed_tasks().await;

        let source_name = arguments
            .as_ref()
            .and_then(|args| args.get("source"))
            .and_then(|v| v.as_str());

        let cancel = arguments
            .as_ref()
            .and_then(|args| args.get("cancel"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let peek = arguments
            .as_ref()
            .and_then(|args| args.get("peek"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let working_dir = self.get_working_dir(session_id).await;

        if source_name.is_none() {
            return self
                .handle_load_discovery(session_id, &working_dir)
                .await
                .map(CallToolResult::success);
        }

        let name = source_name.unwrap();

        if is_session_id(name) {
            let task_result = self
                .handle_load_task_result(name, cancel, peek, notification_emitter)
                .await?;
            let mut meta = MetaObject::new();
            meta.0.insert(
                "subagent_session_id".to_string(),
                serde_json::Value::String(name.to_string()),
            );
            meta.0.insert(
                "task_status".to_string(),
                serde_json::Value::String(task_result.status.to_string()),
            );
            if let Some(turns) = task_result.turns {
                meta.0.insert(
                    "turns_taken".to_string(),
                    serde_json::Value::Number(turns.into()),
                );
            }
            if let Some(secs) = task_result.duration_secs {
                meta.0.insert(
                    "duration_secs".to_string(),
                    serde_json::Value::Number(secs.into()),
                );
            }
            return Ok(CallToolResult::success(task_result.content).with_meta(Some(meta)));
        }

        self.handle_load_source(session_id, name, &working_dir)
            .await
            .map(CallToolResult::success)
    }

    async fn handle_load_task_result(
        &self,
        task_id: &str,
        cancel: bool,
        peek: bool,
        notification_emitter: Option<ToolCallNotificationEmitter>,
    ) -> Result<TaskLoadResult, String> {
        let mut completed = self.completed_tasks.lock().await;

        let completed_entry = completed.get(task_id).map(|task| {
            (
                task.result.clone(),
                task.description.clone(),
                task.duration,
                task.turns_taken,
                Arc::clone(&task.notification_sink),
            )
        });

        if let Some((result, description, duration, turns_taken, notification_sink)) =
            completed_entry
        {
            if !peek {
                Self::attach_notification_emitter(&notification_sink, notification_emitter).await;
                completed.remove(task_id);
            }
            let status_key = match &result {
                Ok(_) => "completed",
                Err(e) if e.starts_with("Task panicked:") => "panicked",
                Err(_) => "failed",
            };
            let status = match status_key {
                "completed" => "✓ Completed",
                "panicked" => "✗ Panicked",
                _ => "✗ Failed",
            };
            let output = match result {
                Ok(output) => output,
                Err(error) => format!("Error: {}", error),
            };
            return Ok(TaskLoadResult {
                content: vec![ContentBlock::text(format!(
                    "# Background Task Result: {}\n\n\
                     **Task:** {}\n\
                     **Status:** {}\n\
                     **Duration:** {} ({} turns)\n\n\
                     ## Output\n\n{}",
                    task_id,
                    description,
                    status,
                    round_duration(duration),
                    turns_taken,
                    output
                ))],
                status: status_key,
                turns: Some(turns_taken),
                duration_secs: Some(duration.as_secs()),
            });
        }

        let mut running = self.background_tasks.lock().await;
        drop(completed);
        if running.contains_key(task_id) {
            if peek {
                let task = running.get(task_id).unwrap();
                let elapsed = task.started_at.elapsed();
                let turns = Arc::clone(&task.turns);
                let last_activity = Arc::clone(&task.last_activity);
                let description = task.description.clone();
                let notification_sink = Arc::clone(&task.notification_sink);

                drop(running);

                let turns_taken = self.refresh_task_turns(task_id, &turns).await;
                let now = current_epoch_millis();
                let last_activity_at = last_activity.load(Ordering::Relaxed);
                let idle_ms = if last_activity_at == 0 {
                    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                } else {
                    now.saturating_sub(last_activity_at)
                };
                let buffered_count = notification_sink.lock().await.buffered_len();

                let mut output = format!(
                    "# Background Task Status: {}\n\n**Task:** {}\n**Status:** ⏳ Running\n**Elapsed:** {}\n**Turns taken:** {}\n**Idle:** {}\n**Buffered tool calls:** {}",
                    task_id,
                    description,
                    round_duration(elapsed),
                    turns_taken,
                    round_duration(Duration::from_millis(idle_ms)),
                    buffered_count,
                );

                if buffered_count == 0 && last_activity_at == 0 {
                    output.push_str("\n\n_Task is initialising (no tool activity yet)._");
                }

                return Ok(TaskLoadResult {
                    content: vec![ContentBlock::text(output)],
                    status: "running",
                    turns: Some(turns_taken),
                    duration_secs: Some(elapsed.as_secs()),
                });
            }

            if cancel {
                let notification_sink =
                    Arc::clone(&running.get(task_id).unwrap().notification_sink);
                Self::attach_notification_emitter(&notification_sink, notification_emitter).await;
                let task = running.remove(task_id).unwrap();
                drop(running);
                task.cancellation_token.cancel();

                let mut handle = task.handle;
                let output = tokio::select! {
                    result = &mut handle => {
                        match result {
                            Ok(Ok(s)) => s,
                            Ok(Err(e)) => format!("Error: {}", e),
                            Err(e) => format!("Task panicked: {}", e),
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        handle.abort();
                        "Task did not stop in time (aborted)".to_string()
                    }
                };
                let duration = task.started_at.elapsed();
                let turns_taken = self.refresh_task_turns(task_id, &task.turns).await;

                return Ok(TaskLoadResult {
                    content: vec![ContentBlock::text(format!(
                        "# Background Task Result: {}\n\n\
                         **Task:** {}\n\
                         **Status:** ⊘ Cancelled\n\
                         **Duration:** {} ({} turns)\n\n\
                         ## Output\n\n{}",
                        task_id,
                        task.description,
                        round_duration(duration),
                        turns_taken,
                        output
                    ))],
                    status: "cancelled",
                    turns: Some(turns_taken),
                    duration_secs: Some(duration.as_secs()),
                });
            }

            // Wait for the running task to complete, keeping the tool call
            // alive so notifications (subagent tool calls) stream in real time.
            let task = running.get(task_id).unwrap();
            let notification_sink = Arc::clone(&task.notification_sink);
            let completion_token = task.completion_token.clone();
            drop(running);
            Self::attach_notification_emitter(&notification_sink, notification_emitter).await;

            tokio::select! {
                _ = self.wait_for_background_task_completion(task_id, &completion_token) => {
                    self.cleanup_completed_tasks().await;
                    return Box::pin(
                        self.handle_load_task_result(task_id, false, false, None)
                    )
                    .await;
                }
                _ = tokio::time::sleep(Duration::from_secs(300)) => {
                    notification_sink.lock().await.detach();

                    return Err(format!(
                        "Task '{task_id}' is still running after waiting 5 min. \
                         Use load(source: \"{task_id}\") to wait again, or \
                         load(source: \"{task_id}\", cancel: true) to stop."
                    ));
                }
            }
        }

        Err(format!("Task '{}' not found.", task_id))
    }

    async fn handle_load_discovery(
        &self,
        session_id: &str,
        working_dir: &Path,
    ) -> Result<Vec<ContentBlock>, String> {
        {
            let mut cache = self.source_cache.lock().await;
            *cache = None;
        }

        let sources = self.get_sources(session_id, working_dir).await;
        let completed = self.completed_tasks.lock().await;

        if sources.is_empty() && completed.is_empty() {
            return Ok(vec![ContentBlock::text(
                "No sources available for load/delegate.\n\n\
                 Sources are discovered from:\n\
                 • Current recipe's sub_recipes\n\
                 • .agents/recipes/, .agents/agents/ (project-level)\n\
                 • ~/.agents/agents/ (global)\n\
                 • GOOSE_RECIPE_PATH directories",
            )]);
        }

        let mut output = String::from("Available sources for load/delegate:\n");

        if !completed.is_empty() {
            output.push_str("\nCompleted Tasks (awaiting retrieval):\n");
            let mut sorted_completed: Vec<_> = completed.values().collect();
            sorted_completed.sort_by_key(|t| &t.id);
            for task in sorted_completed {
                let status = if task.result.is_ok() {
                    "completed"
                } else {
                    "failed"
                };
                output.push_str(&format!(
                    "• {} - \"{}\" ({})\n",
                    task.id, task.description, status
                ));
            }
        }

        for kind in [SourceType::Subrecipe, SourceType::Recipe, SourceType::Agent] {
            let kind_sources: Vec<_> = sources.iter().filter(|s| s.source_type == kind).collect();
            if !kind_sources.is_empty() {
                output.push_str(&format!("\n{}:\n", kind_plural(kind)));
                for source in kind_sources {
                    output.push_str(&format!(
                        "• {} - {}\n",
                        source.name,
                        safe_truncate(&source.description, SUBAGENT_DESCRIPTION_BUDGET)
                    ));
                }
            }
        }

        output.push_str("\nUse load(source: \"name\") to load into context.\n");
        output.push_str("Use delegate(source: \"name\") to run as subagent.");

        Ok(vec![ContentBlock::text(output)])
    }

    async fn handle_load_source(
        &self,
        session_id: &str,
        name: &str,
        working_dir: &Path,
    ) -> Result<Vec<ContentBlock>, String> {
        let source = self.resolve_source(session_id, name, working_dir).await?;

        match source {
            Some(mut source) => {
                if source.source_type == SourceType::Subrecipe && source.content.is_empty() {
                    source.content = self
                        .load_subrecipe_content(session_id, &source.name)
                        .await?;
                }
                let content = source.to_load_text();

                let output = format!(
                    "# Loaded: {} ({})\n\n{}\n\n---\nThis knowledge is now available in your context.",
                    source.name, source.source_type, content
                );

                Ok(vec![ContentBlock::text(output)])
            }
            None => {
                let sources = self.get_sources(session_id, working_dir).await;

                let suggestions: Vec<&str> = sources
                    .iter()
                    .filter(|s| {
                        s.name.to_lowercase().contains(&name.to_lowercase())
                            || name.to_lowercase().contains(&s.name.to_lowercase())
                    })
                    .take(3)
                    .map(|s| s.name.as_str())
                    .collect();

                let error_msg = if suggestions.is_empty() {
                    format!(
                        "Source '{}' not found. Use load() to see available sources.",
                        name
                    )
                } else {
                    format!(
                        "Source '{}' not found. Did you mean: {}?",
                        name,
                        suggestions.join(", ")
                    )
                };

                Err(error_msg)
            }
        }
    }

    async fn handle_delegate(
        &self,
        session_id: &str,
        arguments: Option<JsonObject>,
        cancellation_token: CancellationToken,
        notification_emitter: Option<ToolCallNotificationEmitter>,
    ) -> Result<CallToolResult, String> {
        self.cleanup_completed_tasks().await;

        let params: DelegateParams = arguments
            .map(|args| serde_json::from_value(serde_json::Value::Object(args)))
            .transpose()
            .map_err(|e| format!("Invalid parameters: {}", e))?
            .unwrap_or_default();

        self.validate_delegate_params(&params)?;

        let session = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|e| format!("Failed to get session: {}", e))?;

        if session.session_type == SessionType::SubAgent {
            return Err("Delegated tasks cannot spawn further delegations".to_string());
        }

        if params.r#async {
            let (content, task_id) = self.handle_async_delegate(session_id, params).await?;
            let mut meta = MetaObject::new();
            meta.0.insert(
                "subagent_session_id".to_string(),
                serde_json::Value::String(task_id),
            );
            return Ok(CallToolResult::success(content).with_meta(Some(meta)));
        }

        let working_dir = session.working_dir.clone();
        let recipe = self
            .build_delegate_recipe(&params, session_id, &working_dir)
            .await?;

        let task_config = self
            .build_task_config(&params, &recipe, &session)
            .await
            .map_err(|e| format!("Failed to build task config: {}", e))?;

        // Subagents must use Auto until get_agent_messages forwards
        // ActionRequired messages to the parent. Until then, any mode
        // that requires approval will hang on the subagent's confirmation_rx.
        let mut agent_config = AgentConfig::new(
            self.context.session_manager.clone(),
            crate::config::permission::PermissionManager::instance(),
            None,
            GooseMode::Auto,
            true, // disable session naming for subagents
            crate::agents::GoosePlatform::GooseCli,
        )
        .with_use_login_shell_path(self.context.use_login_shell_path);
        agent_config.is_subagent = true;

        let subagent_session = self
            .create_subagent_session(&task_config, "Delegated task".to_string())
            .await?;

        let subagent_session_id = subagent_session.id.clone();

        let params = SubagentRunParams {
            config: agent_config,
            recipe,
            task_config,
            return_last_only: true,
            session_id: subagent_session.id,
            cancellation_token: Some(cancellation_token),
            on_message: None,
            notification_tx: None,
        };
        let result = Self::run_subagent_with_notifications(
            Self::notification_sink(notification_emitter),
            move |notification_tx| {
                let mut params = params;
                params.notification_tx = Some(notification_tx);
                run_subagent_task(params)
            },
        )
        .await;

        let mut meta = MetaObject::new();
        meta.0.insert(
            "subagent_session_id".to_string(),
            serde_json::Value::String(subagent_session_id),
        );

        match result {
            Ok(text) => {
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]).with_meta(Some(meta)))
            }
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Delegation failed: {}",
                e
            ))])
            .with_meta(Some(meta))),
        }
    }

    fn validate_delegate_params(&self, params: &DelegateParams) -> Result<(), String> {
        if params.instructions.is_none() && params.source.is_none() {
            return Err("Must provide 'instructions' or 'source' (or both)".to_string());
        }

        if params.parameters.is_some() && params.source.is_none() {
            return Err("'parameters' can only be used with 'source'".to_string());
        }

        if let Some(max) = params.max_turns {
            if max < 1 {
                return Err("'max_turns' must be at least 1".to_string());
            }
        }

        Ok(())
    }

    async fn build_delegate_recipe(
        &self,
        params: &DelegateParams,
        session_id: &str,
        working_dir: &Path,
    ) -> Result<Recipe, String> {
        let mut recipe = if let Some(source_name) = &params.source {
            self.build_source_recipe(source_name, params, session_id, working_dir)
                .await?
        } else {
            self.build_adhoc_recipe(params)?
        };

        if let Some(ref context) = params.context {
            let existing = recipe.instructions.unwrap_or_default();
            recipe.instructions = Some(build_instructions_with_context(context, &existing));
        }

        Ok(recipe)
    }

    fn build_adhoc_recipe(&self, params: &DelegateParams) -> Result<Recipe, String> {
        let task = params
            .instructions
            .as_ref()
            .ok_or("Instructions required for ad-hoc task")?;

        Recipe::builder()
            .version("1.0.0")
            .title("Delegated Task")
            .description("Ad-hoc delegated task")
            .prompt(task)
            .build()
            .map_err(|e| format!("Failed to build recipe: {}", e))
    }

    async fn build_source_recipe(
        &self,
        source_name: &str,
        params: &DelegateParams,
        session_id: &str,
        working_dir: &Path,
    ) -> Result<Recipe, String> {
        let source = self
            .resolve_source(session_id, source_name, working_dir)
            .await?
            .ok_or_else(|| format!("Source '{}' not found", source_name))?;

        let mut recipe = match source.source_type {
            SourceType::Recipe | SourceType::Subrecipe => {
                self.build_recipe_from_source(&source, params, session_id)
                    .await?
            }
            SourceType::Agent => self.build_recipe_from_agent(&source, params)?,
            _ => {
                return Err(format!(
                    "Source '{}' has kind '{}' which cannot be delegated from summon",
                    source_name, source.source_type
                ));
            }
        };

        if let Some(extra_instructions) = &params.instructions {
            if recipe.prompt.is_some() {
                let current_prompt = recipe.prompt.take().unwrap();
                recipe.prompt = Some(format!("{}\n\n{}", current_prompt, extra_instructions));
            } else {
                recipe.prompt = Some(extra_instructions.clone());
            }
        }

        Ok(recipe)
    }

    async fn build_recipe_from_source(
        &self,
        source: &SourceEntry,
        params: &DelegateParams,
        session_id: &str,
    ) -> Result<Recipe, String> {
        let session = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|e| format!("Failed to get session: {}", e))?;

        if source.source_type == SourceType::Subrecipe {
            let sub_recipes = session.recipe.as_ref().and_then(|r| r.sub_recipes.as_ref());

            if let Some(sub_recipes) = sub_recipes {
                if let Some(sr) = sub_recipes.iter().find(|sr| sr.name == source.name) {
                    let recipe_file = load_local_recipe_file(&sr.path).map_err(|e| {
                        format!("Failed to load subrecipe '{}': {}", source.name, e)
                    })?;

                    let merged =
                        merge_subrecipe_parameters(sr.values.as_ref(), params.parameters.as_ref());
                    let param_values: Vec<(String, String)> = merged.into_iter().collect();

                    return build_recipe_from_template(
                        recipe_file.content,
                        &recipe_file.parent_dir,
                        param_values,
                        None::<fn(&str, &str) -> Result<String, anyhow::Error>>,
                    )
                    .map_err(|e| format!("Failed to build subrecipe: {}", e));
                }
            }
        }

        let recipe_file = load_local_recipe_file(&source.path)
            .map_err(|e| format!("Failed to load recipe '{}': {}", source.name, e))?;

        let param_values: Vec<(String, String)> = params
            .parameters
            .as_ref()
            .map(|p| {
                p.iter()
                    .map(|(k, v)| {
                        let value_str = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), value_str)
                    })
                    .collect()
            })
            .unwrap_or_default();

        build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            param_values,
            None::<fn(&str, &str) -> Result<String, anyhow::Error>>,
        )
        .map_err(|e| format!("Failed to build recipe: {}", e))
    }

    fn build_recipe_from_agent(
        &self,
        source: &SourceEntry,
        params: &DelegateParams,
    ) -> Result<Recipe, String> {
        if source.path.is_empty() {
            return Err("Agent source has no path".to_string());
        }

        let model = source
            .properties
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        // max_turns is set later in build_task_config so it can incorporate params.max_turns
        // with the correct priority ordering; setting it here would cause it to be overridden
        // by the parent session's recipe instead.
        let settings = model.map(|m| Settings {
            goose_model: Some(m),
            goose_provider: params.provider.clone(),
            temperature: params.temperature,
            max_turns: None,
        });

        let mut builder = Recipe::builder()
            .version("1.0.0")
            .title(format!("Agent: {}", source.name))
            .description(source.description.clone())
            .instructions(&source.content);

        if let Some(settings) = settings {
            builder = builder.settings(settings);
        }

        if params.instructions.is_none() {
            builder = builder.prompt("Proceed with your expertise to produce a useful result.");
        }

        builder
            .build()
            .map_err(|e| format!("Failed to build recipe from agent: {}", e))
    }

    async fn build_task_config(
        &self,
        params: &DelegateParams,
        recipe: &Recipe,
        session: &crate::session::Session,
    ) -> Result<TaskConfig, anyhow::Error> {
        let mut extensions = EnabledExtensionsState::extensions_or_default(
            Some(&session.extension_data),
            Config::global(),
        );

        if let Some(filter) = &params.extensions {
            if filter.is_empty() {
                extensions = Vec::new();
            } else {
                let available_names: Vec<String> =
                    extensions.iter().map(|ext| ext.name()).collect();
                extensions.retain(|ext| filter.contains(&ext.name()));
                let unmatched: Vec<&str> = filter
                    .iter()
                    .filter(|name| !available_names.iter().any(|n| n == *name))
                    .map(String::as_str)
                    .collect();
                if !unmatched.is_empty() {
                    warn!(
                        "Delegate requested extensions not available in session: {:?}. Available: {:?}",
                        unmatched, available_names
                    );
                }
            }
        }

        let (provider, model_config) = self
            .resolve_provider(params, recipe, session, &extensions)
            .await?;

        let max_turns = params
            .max_turns
            .or_else(|| recipe.settings.as_ref().and_then(|s| s.max_turns))
            .unwrap_or_else(|| self.resolve_max_turns(session));

        if max_turns == 0 || max_turns > u32::MAX as usize {
            anyhow::bail!(
                "max_turns must be between 1 and {} (got {})",
                u32::MAX,
                max_turns
            );
        }

        let effective_working_dir = match &params.working_dir {
            Some(dir) => resolve_working_dir(&session.working_dir, dir)?,
            None => session.working_dir.clone(),
        };

        let task_config = TaskConfig::new(
            provider,
            model_config,
            &session.id,
            &effective_working_dir,
            extensions,
        )
        .with_max_turns(Some(max_turns));

        Ok(task_config)
    }

    fn resolve_model_config(
        &self,
        params: &DelegateParams,
        recipe: &Recipe,
        session: &crate::session::Session,
        provider_name: &str,
        provider_default_model: Option<&str>,
    ) -> Result<goose_providers::model::ModelConfig, anyhow::Error> {
        let env_model = std::env::var("GOOSE_SUBAGENT_MODEL").ok();
        let recipe_settings = recipe.settings.as_ref();
        let configured = Config::global().all_values().ok();
        let configured_provider = configured
            .as_ref()
            .and_then(|values| values.get("GOOSE_SUBAGENT_PROVIDER"))
            .and_then(serde_json::Value::as_str);
        let configured_model = configured
            .as_ref()
            .and_then(|values| values.get("GOOSE_SUBAGENT_MODEL"))
            .and_then(serde_json::Value::as_str);
        let matches_provider =
            |candidate: Option<&str>| candidate.is_none() || candidate == Some(provider_name);
        let model = env_model
            .or_else(|| {
                params
                    .model
                    .clone()
                    .filter(|_| matches_provider(params.provider.as_deref()))
            })
            .or_else(|| {
                recipe_settings
                    .and_then(|settings| settings.goose_model.clone())
                    .filter(|_| {
                        matches_provider(
                            recipe_settings.and_then(|settings| settings.goose_provider.as_deref()),
                        )
                    })
            })
            .or_else(|| {
                configured_model
                    .filter(|_| matches_provider(configured_provider))
                    .map(str::to_string)
            })
            .or_else(|| {
                session
                    .model_config
                    .as_ref()
                    .filter(|_| matches_provider(session.provider_name.as_deref()))
                    .map(|config| config.model_name.clone())
            })
            .or_else(|| {
                provider_default_model
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No model configured for provider '{}'; set GOOSE_SUBAGENT_MODEL",
                    provider_name
                )
            })?;

        let parent = session.model_config.as_ref();
        let mut model_config = if parent.is_some_and(|config| {
            matches_provider(session.provider_name.as_deref()) && config.model_name == model
        }) {
            parent.unwrap().clone()
        } else {
            let mut cfg = crate::model_config::model_config_from_user_config_with_session_settings(
                provider_name,
                &model,
                parent,
                None,
                None,
            )?;
            if let Some(parent) = parent {
                cfg.toolshim = parent.toolshim;
                cfg.toolshim_model = parent.toolshim_model.clone();
                cfg.temperature = cfg.temperature.or(parent.temperature);
            }
            cfg
        };

        if let Some(temp) = params.temperature {
            model_config = model_config.with_temperature(Some(temp));
        } else if let Some(temp) = recipe.settings.as_ref().and_then(|s| s.temperature) {
            model_config = model_config.with_temperature(Some(temp));
        }

        Ok(model_config)
    }

    async fn resolve_provider(
        &self,
        params: &DelegateParams,
        recipe: &Recipe,
        session: &crate::session::Session,
        extensions: &[crate::config::ExtensionConfig],
    ) -> Result<
        (
            Arc<dyn crate::providers::base::Provider>,
            goose_providers::model::ModelConfig,
        ),
        anyhow::Error,
    > {
        let env_provider = std::env::var("GOOSE_SUBAGENT_PROVIDER").ok();
        let provider_name = env_provider
            .clone()
            .or_else(|| params.provider.clone())
            .or_else(|| {
                recipe
                    .settings
                    .as_ref()
                    .and_then(|s| s.goose_provider.clone())
            })
            .or_else(|| {
                Config::global()
                    .get_param::<String>("GOOSE_SUBAGENT_PROVIDER")
                    .ok()
            })
            .or_else(|| session.provider_name.clone())
            .ok_or_else(|| anyhow::anyhow!("No provider configured"))?;

        let provider_entry = providers::get_from_registry(&provider_name).await;
        let provider_default_model = provider_entry
            .as_ref()
            .ok()
            .map(|entry| entry.metadata().default_model.as_str());
        let model_config = self.resolve_model_config(
            params,
            recipe,
            session,
            &provider_name,
            provider_default_model,
        )?;
        let provider = match provider_entry {
            Ok(entry) => entry.create(extensions.to_vec()).await?,
            Err(error) => {
                let parent_provider = if let Some(extension_manager) = self
                    .context
                    .extension_manager
                    .as_ref()
                    .and_then(|weak| weak.upgrade())
                {
                    extension_manager.get_provider().lock().await.clone()
                } else {
                    None
                };

                match parent_provider {
                    Some(provider)
                        if provider.get_name() == provider_name
                            && !provider.manages_own_context() =>
                    {
                        provider
                    }
                    _ => return Err(error),
                }
            }
        };
        Ok((provider, model_config))
    }

    fn resolve_max_turns(&self, session: &crate::session::Session) -> usize {
        session
            .recipe
            .as_ref()
            .and_then(|r| r.settings.as_ref())
            .and_then(|s| s.max_turns)
            .or_else(|| {
                std::env::var("GOOSE_SUBAGENT_MAX_TURNS")
                    .ok()
                    .and_then(|v| v.parse().ok())
            })
            .or_else(|| {
                Config::global()
                    .get_param::<usize>("GOOSE_SUBAGENT_MAX_TURNS")
                    .ok()
            })
            .unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS)
    }

    /// Count durable, user-visible assistant blocks in the active task turn,
    /// excluding assistant-only compaction scaffolding.
    async fn refresh_task_turns(&self, task_id: &str, cached_turns: &AtomicU32) -> u32 {
        match self
            .context
            .session_manager
            .get_session(task_id, true)
            .await
        {
            Ok(session) => {
                let turns = session
                    .conversation
                    .as_ref()
                    .map(durable_assistant_turn_count)
                    .unwrap_or_default();
                cached_turns.store(turns, Ordering::Relaxed);
                turns
            }
            Err(error) => {
                warn!(
                    "Failed to refresh turn count for background task {}: {}",
                    task_id, error
                );
                cached_turns.load(Ordering::Relaxed)
            }
        }
    }

    async fn refresh_running_task_turns(&self) -> HashMap<String, u32> {
        let tasks: Vec<_> = self
            .background_tasks
            .lock()
            .await
            .values()
            .map(|task| (task.id.clone(), Arc::clone(&task.turns)))
            .collect();
        let mut refreshed = HashMap::with_capacity(tasks.len());
        for (id, turns) in tasks {
            let count = self.refresh_task_turns(&id, &turns).await;
            refreshed.insert(id, count);
        }
        refreshed
    }

    async fn wait_for_background_task_completion(
        &self,
        task_id: &str,
        completion_token: &CancellationToken,
    ) {
        completion_token.cancelled().await;
        loop {
            let finished_or_moved = self
                .background_tasks
                .lock()
                .await
                .get(task_id)
                .map(|task| task.handle.is_finished())
                .unwrap_or(true);
            if finished_or_moved {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    async fn cleanup_completed_tasks(&self) {
        let finished: Vec<(String, Arc<AtomicU32>)> = self
            .background_tasks
            .lock()
            .await
            .iter()
            .filter(|(_, task)| task.handle.is_finished())
            .map(|(id, task)| (id.clone(), Arc::clone(&task.turns)))
            .collect();

        let mut refreshed = HashMap::with_capacity(finished.len());
        for (id, turns) in &finished {
            let count = self.refresh_task_turns(id, turns).await;
            refreshed.insert(id.clone(), count);
        }

        // Keep the same lock order as task lookup so the running -> completed
        // transition is atomic from callers' perspective.
        let mut completed = self.completed_tasks.lock().await;
        let mut tasks = self.background_tasks.lock().await;
        for (id, _) in finished {
            let Some(task) = tasks.remove(&id) else {
                continue;
            };
            let turns_taken = refreshed
                .remove(&id)
                .unwrap_or_else(|| task.turns.load(Ordering::Relaxed));
            let duration = task.started_at.elapsed();

            let result = match task.handle.await {
                Ok(Ok(output)) => {
                    info!("Background task {} completed successfully", id);
                    Ok(output)
                }
                Ok(Err(e)) => {
                    warn!("Background task {} failed: {}", id, e);
                    Err(e.to_string())
                }
                Err(e) => {
                    warn!("Background task {} panicked: {}", id, e);
                    Err(format!("Task panicked: {}", e))
                }
            };

            completed.insert(
                id.clone(),
                CompletedTask {
                    id,
                    description: task.description,
                    result,
                    turns_taken,
                    duration,
                    completed_at: Instant::now(),
                    notification_sink: task.notification_sink,
                },
            );
        }

        let ttl = completed_task_ttl();
        completed.retain(|_id, task| task.completed_at.elapsed() <= ttl);
    }

    fn get_task_description(params: &DelegateParams) -> String {
        match (&params.source, &params.instructions) {
            (Some(source), Some(instructions)) => format!("{}: {}", source, instructions),
            (Some(source), None) => source.clone(),
            (None, Some(instructions)) => instructions.clone(),
            (None, None) => "Unknown task".to_string(),
        }
    }

    async fn handle_async_delegate(
        &self,
        session_id: &str,
        params: DelegateParams,
    ) -> Result<(Vec<ContentBlock>, String), String> {
        let task_count = self.background_tasks.lock().await.len();
        let max_tasks = max_background_tasks();
        if task_count >= max_tasks {
            return Err(format!(
                "Maximum {} background tasks already running. Wait for completion or use sync mode.",
                max_tasks
            ));
        }

        let session = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .map_err(|e| format!("Failed to get session: {}", e))?;

        let working_dir = session.working_dir.clone();
        let recipe = self
            .build_delegate_recipe(&params, session_id, &working_dir)
            .await?;

        let task_config = self
            .build_task_config(&params, &recipe, &session)
            .await
            .map_err(|e| format!("Failed to build task config: {}", e))?;

        let description = safe_truncate(&Self::get_task_description(&params), TASK_LABEL_BUDGET);

        // Subagents must use Auto until get_agent_messages forwards
        // ActionRequired messages to the parent. Until then, any mode
        // that requires approval will hang on the subagent's confirmation_rx.
        let mut agent_config = AgentConfig::new(
            self.context.session_manager.clone(),
            crate::config::permission::PermissionManager::instance(),
            None,
            GooseMode::Auto,
            true, // disable session naming for subagents
            crate::agents::GoosePlatform::GooseCli,
        )
        .with_use_login_shell_path(self.context.use_login_shell_path);
        agent_config.is_subagent = true;

        let subagent_session = self
            .create_subagent_session(&task_config, description.clone())
            .await?;

        let task_id = subagent_session.id.clone();

        let turns = Arc::new(AtomicU32::new(0));
        let last_activity = Arc::new(AtomicU64::new(0));

        let last_activity_clone = Arc::clone(&last_activity);

        let on_message: OnMessageCallback = Arc::new(move |_msg| {
            last_activity_clone.store(current_epoch_millis(), Ordering::Relaxed);
        });

        let task_token = CancellationToken::new();
        let task_token_clone = task_token.clone();

        let notification_sink = Self::notification_sink(None);
        let task_notification_sink = Arc::clone(&notification_sink);

        let (handle, completion_token) = spawn_background_task(async move {
            let params = SubagentRunParams {
                config: agent_config,
                recipe,
                task_config,
                return_last_only: true,
                session_id: subagent_session.id,
                cancellation_token: Some(task_token_clone),
                on_message: Some(on_message),
                notification_tx: None,
            };
            Self::run_subagent_with_notifications(task_notification_sink, move |notification_tx| {
                let mut params = params;
                params.notification_tx = Some(notification_tx);
                run_subagent_task(params)
            })
            .await
        });

        let task = BackgroundTask {
            id: task_id.clone(),
            description: description.clone(),
            started_at: Instant::now(),
            turns,
            last_activity,
            handle,
            cancellation_token: task_token,
            completion_token,
            notification_sink,
        };

        self.background_tasks
            .lock()
            .await
            .insert(task_id.clone(), task);

        let content = vec![ContentBlock::text(format!(
            "Task {} started in background: \"{}\"\n\
             Continue with other work. When you need the result, use load(source: \"{}\").",
            task_id, description, task_id
        ))];
        Ok((content, task_id))
    }
}

#[async_trait]
impl McpClientTrait for SummonClient {
    async fn list_tools(
        &self,
        session_id: &str,
        _next_cursor: Option<String>,
        _cancellation_token: CancellationToken,
    ) -> Result<ListToolsResult, Error> {
        self.cleanup_completed_tasks().await;

        let is_subagent = self
            .context
            .session_manager
            .get_session(session_id, false)
            .await
            .map(|s| s.session_type == SessionType::SubAgent)
            .unwrap_or(false);

        let mut tools = vec![self.create_load_tool()];

        if !is_subagent {
            tools.push(self.create_delegate_tool());
        }

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        ctx: &ToolCallContext,
        name: &str,
        arguments: Option<JsonObject>,
        cancellation_token: CancellationToken,
    ) -> Result<CallToolResult, Error> {
        let session_id = &ctx.session_id;
        match name {
            "load" => match self
                .handle_load(session_id, arguments, ctx.notification_emitter().cloned())
                .await
            {
                Ok(result) => Ok(result),
                Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "Error: {}",
                    error
                ))])),
            },
            "delegate" => {
                match self
                    .handle_delegate(
                        session_id,
                        arguments,
                        cancellation_token,
                        ctx.notification_emitter().cloned(),
                    )
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "Error: {}",
                        error
                    ))])),
                }
            }
            _ => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Error: Unknown tool: {}",
                name
            ))])),
        }
    }

    fn get_info(&self) -> Option<&InitializeResult> {
        Some(&self.info)
    }

    fn get_instructions(&self) -> Option<String> {
        let instructions = build_subagent_instructions(self.context.session.as_deref());
        if instructions.is_empty() {
            None
        } else {
            Some(instructions)
        }
    }

    async fn get_moim(&self, _session_id: &str) -> Option<String> {
        self.cleanup_completed_tasks().await;
        let refreshed_turns = self.refresh_running_task_turns().await;

        let completed = self.completed_tasks.lock().await;
        let running = self.background_tasks.lock().await;

        if running.is_empty() && completed.is_empty() {
            return None;
        }

        let mut lines = vec!["Background tasks:".to_string()];
        let now = current_epoch_millis();

        let mut sorted_running: Vec<_> = running.values().collect();
        sorted_running.sort_by_key(|task| &task.id);

        for task in sorted_running {
            let elapsed = task.started_at.elapsed();
            let last_activity_at = task.last_activity.load(Ordering::Relaxed);
            let idle_ms = if last_activity_at == 0 {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            } else {
                now.saturating_sub(last_activity_at)
            };

            lines.push(format!(
                "• {}: \"{}\" - running {}, {} turns, idle {}",
                task.id,
                task.description,
                round_duration(elapsed),
                refreshed_turns
                    .get(&task.id)
                    .copied()
                    .unwrap_or_else(|| task.turns.load(Ordering::Relaxed)),
                round_duration(Duration::from_millis(idle_ms)),
            ));
        }

        let mut sorted_completed: Vec<_> = completed.values().collect();
        sorted_completed.sort_by_key(|task| &task.id);

        for task in sorted_completed {
            let status = if task.result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            lines.push(format!(
                "• {}: \"{}\" - {} in {} ({} turns) - use load(\"{}\") to get result",
                task.id,
                task.description,
                status,
                round_duration(task.duration),
                task.turns_taken,
                task.id
            ));
        }

        if !running.is_empty() {
            lines.push(
                "\n→ Use load(source: \"<id>\") to wait for a task, or load(source: \"<id>\", cancel: true) to stop it"
                    .to_string(),
            );
        }

        Some(lines.join("\n"))
    }
}

/// Resolve a requested `working_dir` override against the parent session
/// directory. Relative paths are joined to the parent dir; the result must
/// canonicalize to an existing directory contained within the parent dir.
fn resolve_working_dir(parent_dir: &Path, requested: &str) -> Result<PathBuf, anyhow::Error> {
    let requested_path = PathBuf::from(requested);
    let resolved = if requested_path.is_absolute() {
        requested_path
    } else {
        parent_dir.join(&requested_path)
    };
    let canonical = resolved
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("working_dir '{}' could not be resolved: {}", requested, e))?;
    let parent_canonical = parent_dir
        .canonicalize()
        .unwrap_or_else(|_| parent_dir.to_path_buf());
    if !canonical.starts_with(&parent_canonical) {
        anyhow::bail!(
            "working_dir '{}' is outside the parent session directory",
            requested
        );
    }
    if !canonical.is_dir() {
        anyhow::bail!("working_dir '{}' is not a directory", requested);
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use futures::StreamExt;
    use serial_test::serial;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn create_test_context() -> PlatformExtensionContext {
        create_test_context_with_session_manager(Arc::new(
            crate::session::SessionManager::instance(),
        ))
    }

    fn create_test_context_with_session_manager(
        session_manager: Arc<crate::session::SessionManager>,
    ) -> PlatformExtensionContext {
        PlatformExtensionContext {
            extension_manager: None,
            session_manager,
            scheduler: None,
            session: None,
            use_login_shell_path: false,
        }
    }

    async fn create_test_subagent_session(
        session_manager: &crate::session::SessionManager,
        working_dir: &Path,
        messages: &[Message],
    ) -> String {
        let session = session_manager
            .create_session(
                working_dir.to_path_buf(),
                "Background task".to_string(),
                SessionType::SubAgent,
                GooseMode::Auto,
            )
            .await
            .unwrap();
        for message in messages {
            session_manager
                .add_message(&session.id, message)
                .await
                .unwrap();
        }
        session.id
    }

    #[test]
    fn test_agent_frontmatter_parsing() {
        let agent = r#"---
name: reviewer
model: sonnet
---
You review code."#;
        let source = parse_agent_content(agent, Path::new("")).unwrap();
        assert_eq!(source.name, "reviewer");
        assert!(source.description.contains("sonnet"));
        assert_eq!(
            source
                .properties
                .get("model")
                .and_then(|value| value.as_str()),
            Some("sonnet")
        );
    }

    #[test]
    fn test_resolve_working_dir_relative_subdir() {
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().canonicalize().unwrap();
        let subdir = parent.join("sub");
        fs::create_dir(&subdir).unwrap();

        let resolved = resolve_working_dir(&parent, "sub").unwrap();
        assert_eq!(resolved, subdir.canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_working_dir_rejects_traversal_outside_parent() {
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().join("parent");
        let sibling = temp_dir.path().join("sibling");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&sibling).unwrap();

        let err = resolve_working_dir(&parent, "../sibling").unwrap_err();
        assert!(
            err.to_string()
                .contains("outside the parent session directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_resolve_working_dir_rejects_file_path() {
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().canonicalize().unwrap();
        let file = parent.join("a.txt");
        fs::write(&file, "hello").unwrap();

        let err = resolve_working_dir(&parent, "a.txt").unwrap_err();
        assert!(
            err.to_string().contains("is not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_resolve_working_dir_rejects_nonexistent_path() {
        let temp_dir = TempDir::new().unwrap();
        let parent = temp_dir.path().canonicalize().unwrap();

        let err = resolve_working_dir(&parent, "does-not-exist").unwrap_err();
        assert!(
            err.to_string().contains("could not be resolved"),
            "unexpected error: {err}"
        );
    }
    #[test]
    fn test_agent_scan_skips_non_agent_markdown() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            agents_dir.join("README.md"),
            "---\ntitle: Notes\n---\nThis is not an agent.",
        )
        .unwrap();
        fs::write(
            agents_dir.join("notes.md"),
            "---\nauthor: someone\ntags: [docs]\n---\nJust documentation.",
        )
        .unwrap();
        fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\nmodel: sonnet\n---\nYou review code.",
        )
        .unwrap();
        fs::write(agents_dir.join("plain.md"), "No frontmatter at all.").unwrap();
        fs::write(
            agents_dir.join("broken.md"),
            "---\nname: [unterminated\n---\nBroken YAML.",
        )
        .unwrap();

        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        scan_agents_from_dir(&agents_dir, &mut sources, &mut seen);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "reviewer");
    }

    #[cfg(unix)]
    #[test]
    fn agent_scan_rejects_symlinked_source_file() {
        let temp_dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(
            outside.path().join("outside.md"),
            "---\nname: outside\n---\nUntrusted agent.",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.md"),
            temp_dir.path().join("outside.md"),
        )
        .unwrap();

        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        scan_agents_from_dir(temp_dir.path(), &mut sources, &mut seen);

        assert!(sources.is_empty());
    }

    #[test]
    fn test_recipe_scan_skips_non_recipe_project_config_files() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"test":"cargo test"}}"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"strict":true}}"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("valid.yaml"),
            "title: Valid\ndescription: Real recipe\ninstructions: Run valid steps",
        )
        .unwrap();

        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        scan_recipes_from_dir(
            temp_dir.path(),
            SourceType::Recipe,
            true,
            &mut sources,
            &mut seen,
        );

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "valid");
        assert_eq!(sources[0].description, "Real recipe");
    }

    #[cfg(unix)]
    #[test]
    fn recipe_scan_rejects_symlinked_source_file() {
        let temp_dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(
            outside.path().join("outside.yaml"),
            "title: Outside\ndescription: Outside recipe\ninstructions: Untrusted",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.yaml"),
            temp_dir.path().join("outside.yaml"),
        )
        .unwrap();

        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        scan_recipes_from_dir(
            temp_dir.path(),
            SourceType::Recipe,
            false,
            &mut sources,
            &mut seen,
        );

        assert!(sources.is_empty());
    }

    #[tokio::test]
    async fn test_discover_recipes_and_agents() {
        let temp_dir = TempDir::new().unwrap();

        let recipes = temp_dir.path().join(".goose/recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(
            recipes.join("deploy.yaml"),
            "title: Deploy\ndescription: Deploy to production\ninstructions: Run deploy steps",
        )
        .unwrap();

        let agents = temp_dir.path().join(".goose/agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\nmodel: sonnet\ndescription: Code reviewer\n---\nYou review code.",
        )
        .unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let sources = client.discover_filesystem_sources(temp_dir.path());

        let recipe = sources
            .iter()
            .find(|s| s.name == "deploy" && s.source_type == SourceType::Recipe)
            .unwrap();
        assert_eq!(recipe.description, "Deploy to production");
        assert_eq!(recipe.content, "Run deploy steps");

        let agent = sources
            .iter()
            .find(|s| s.name == "reviewer" && s.source_type == SourceType::Agent)
            .unwrap();
        assert_eq!(agent.description, "Code reviewer");
        assert!(agent.content.contains("You review code"));
    }

    #[tokio::test]
    async fn test_recipe_deduplication_local_wins() {
        let temp_dir = TempDir::new().unwrap();

        let local = temp_dir.path().join(".goose/recipes");
        fs::create_dir_all(&local).unwrap();
        fs::write(
            local.join("deploy.yaml"),
            "title: Deploy\ndescription: Local deploy\ninstructions: local steps",
        )
        .unwrap();

        let also_local = temp_dir.path().join(".agents/recipes");
        fs::create_dir_all(&also_local).unwrap();
        fs::write(
            also_local.join("deploy.yaml"),
            "title: Deploy\ndescription: Agents deploy\ninstructions: agents steps",
        )
        .unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let sources = client.discover_filesystem_sources(temp_dir.path());

        let deploys: Vec<_> = sources.iter().filter(|s| s.name == "deploy").collect();
        assert_eq!(deploys.len(), 1);
    }

    #[tokio::test]
    async fn test_load_recipe_source() {
        let temp_dir = TempDir::new().unwrap();

        let recipes = temp_dir.path().join(".goose/recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(
            recipes.join("deploy.yaml"),
            "title: Deploy\ndescription: Deploy to production\ninstructions: Run deploy steps",
        )
        .unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let result = client
            .handle_load_source("test", "deploy", temp_dir.path())
            .await
            .unwrap();

        let text = &result[0].as_text().expect("expected text content").text;
        assert!(text.contains("deploy"));
        assert!(text.contains("Run deploy steps"));
        assert!(text.contains("now available in your context"));
    }

    #[test]
    fn test_invalid_external_subrecipe_content_is_not_returned() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("invalid.yaml");
        fs::write(&path, "api_key: SUPERSECRET\n").unwrap();

        let recipe_file = load_local_recipe_file(path.to_str().unwrap()).unwrap();
        let error =
            SummonClient::format_subrecipe_content("invalid", &recipe_file.content).unwrap_err();

        assert_eq!(error, "Subrecipe 'invalid' is not a valid recipe");
        assert!(!error.contains("SUPERSECRET"));
    }

    #[test]
    fn test_valid_external_subrecipe_content_still_loads() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("child.yaml");
        fs::write(
            &path,
            "title: Child\ndescription: External child\ninstructions: Run child steps",
        )
        .unwrap();

        let recipe_file = load_local_recipe_file(path.to_str().unwrap()).unwrap();
        let content =
            SummonClient::format_subrecipe_content("child", &recipe_file.content).unwrap();

        assert_eq!(content, "Run child steps");
    }

    #[tokio::test]
    async fn test_load_agent_source() {
        let temp_dir = TempDir::new().unwrap();

        let agents = temp_dir.path().join(".goose/agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\nmodel: sonnet\ndescription: Code reviewer\n---\nYou review code carefully.",
        )
        .unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let result = client
            .handle_load_source("test", "reviewer", temp_dir.path())
            .await
            .unwrap();

        let text = &result[0].as_text().expect("expected text content").text;
        assert!(text.contains("reviewer"));
        assert!(text.contains("You review code carefully"));
        assert!(text.contains("now available in your context"));
    }

    #[tokio::test]
    async fn test_load_nonexistent_source_suggests_similar() {
        let temp_dir = TempDir::new().unwrap();

        let recipes = temp_dir.path().join(".goose/recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(
            recipes.join("deploy.yaml"),
            "title: Deploy\ndescription: Deploy to production\ninstructions: steps",
        )
        .unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let err = client
            .handle_load_source("test", "deploy-prod", temp_dir.path())
            .await
            .unwrap_err();

        assert!(err.contains("not found"));
        assert!(err.contains("deploy"), "should suggest 'deploy': {}", err);
    }

    #[tokio::test]
    async fn test_load_completely_unknown_source() {
        let temp_dir = TempDir::new().unwrap();

        let client = SummonClient::new(create_test_context()).unwrap();
        let err = client
            .handle_load_source("test", "zzz-nonexistent", temp_dir.path())
            .await
            .unwrap_err();

        assert!(err.contains("not found"));
        assert!(err.contains("Use load()"));
    }

    #[tokio::test]
    async fn test_client_tools_and_unknown_tool() {
        let client = SummonClient::new(create_test_context()).unwrap();

        let result = client
            .list_tools("test", None, CancellationToken::new())
            .await
            .unwrap();
        let names: Vec<_> = result.tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"load") && names.contains(&"delegate"));

        let ctx = ToolCallContext::new("test".to_string(), None, None);
        let result = client
            .call_tool(&ctx, "unknown", None, CancellationToken::new())
            .await
            .unwrap();
        assert!(result.is_error.unwrap_or(false));
    }

    #[test]
    fn test_duration_rounding_for_moim() {
        assert_eq!(round_duration(Duration::from_secs(5)), "0s");
        assert_eq!(round_duration(Duration::from_secs(15)), "10s");
        assert_eq!(round_duration(Duration::from_secs(59)), "50s");

        assert_eq!(round_duration(Duration::from_secs(60)), "1m");
        assert_eq!(round_duration(Duration::from_secs(90)), "1m");
        assert_eq!(round_duration(Duration::from_secs(120)), "2m");
    }

    #[test]
    fn test_task_description_formatting() {
        let make_params = |source: Option<&str>, instructions: Option<&str>| DelegateParams {
            source: source.map(String::from),
            instructions: instructions.map(String::from),
            ..Default::default()
        };

        assert_eq!(
            SummonClient::get_task_description(&make_params(Some("recipe"), None)),
            "recipe"
        );
        assert_eq!(
            SummonClient::get_task_description(&make_params(None, Some("do stuff"))),
            "do stuff"
        );
        assert_eq!(
            SummonClient::get_task_description(&make_params(Some("r"), Some("task"))),
            "r: task"
        );
        assert_eq!(
            SummonClient::get_task_description(&make_params(None, None)),
            "Unknown task"
        );
    }

    #[tokio::test]
    async fn test_context_injected_into_adhoc_recipe() {
        let temp_dir = TempDir::new().unwrap();
        let client = SummonClient::new(create_test_context()).unwrap();

        let params = DelegateParams {
            instructions: Some("do the task".to_string()),
            context: Some("background info".to_string()),
            ..Default::default()
        };

        let recipe = client
            .build_delegate_recipe(&params, "test", temp_dir.path())
            .await
            .unwrap();

        assert_eq!(
            recipe.instructions.as_deref(),
            Some("# Reference Context\n\nbackground info")
        );
        assert_eq!(recipe.prompt.as_deref(), Some("do the task"));
    }

    #[test]
    fn test_subrecipe_fixed_values_take_precedence_over_delegate_parameters() {
        let fixed = HashMap::from([("fixed".to_string(), "parent-value".to_string())]);
        let provided = HashMap::from([
            (
                "fixed".to_string(),
                serde_json::Value::String("delegate-value".to_string()),
            ),
            (
                "caller".to_string(),
                serde_json::Value::String("caller-value".to_string()),
            ),
        ]);

        let merged = merge_subrecipe_parameters(Some(&fixed), Some(&provided));

        assert_eq!(
            merged.get("fixed").map(String::as_str),
            Some("parent-value")
        );
        assert_eq!(
            merged.get("caller").map(String::as_str),
            Some("caller-value")
        );
    }

    #[test]
    fn test_build_instructions_with_context_wraps_existing_instructions() {
        assert_eq!(
            build_instructions_with_context("background info", "Run deploy steps"),
            "# Reference Context\n\nbackground info\n\n# Task Instructions\n\nRun deploy steps"
        );
        assert_eq!(
            build_instructions_with_context("background info", ""),
            "# Reference Context\n\nbackground info"
        );
    }

    #[test]
    fn test_validate_delegate_params_rejects_zero_max_turns() {
        let context = create_test_context();
        let client = SummonClient::new(context).unwrap();

        let params = DelegateParams {
            instructions: Some("do something".to_string()),
            max_turns: Some(0),
            ..Default::default()
        };
        let result = client.validate_delegate_params(&params);
        assert_eq!(result, Err("'max_turns' must be at least 1".to_string()));
    }

    #[test]
    fn test_validate_delegate_params_accepts_positive_max_turns() {
        let context = create_test_context();
        let client = SummonClient::new(context).unwrap();

        let params = DelegateParams {
            instructions: Some("do something".to_string()),
            max_turns: Some(5),
            ..Default::default()
        };
        assert!(client.validate_delegate_params(&params).is_ok());
    }

    #[test]
    #[serial]
    fn test_resolve_max_turns_recipe_overrides_env_var() {
        let context = create_test_context();
        let client = SummonClient::new(context).unwrap();

        let session = crate::session::Session {
            recipe: Some(crate::recipe::Recipe {
                version: "1.0.0".to_string(),
                title: String::new(),
                description: String::new(),
                instructions: None,
                prompt: None,
                extensions: None,
                settings: Some(crate::recipe::Settings {
                    goose_provider: None,
                    goose_model: None,
                    temperature: None,
                    max_turns: Some(10),
                }),
                activities: None,
                author: None,
                parameters: None,
                response: None,
                sub_recipes: None,
                retry: None,
            }),
            ..Default::default()
        };

        // Set env var to a different value — recipe should still win
        std::env::set_var("GOOSE_SUBAGENT_MAX_TURNS", "99");
        let result = client.resolve_max_turns(&session);
        std::env::remove_var("GOOSE_SUBAGENT_MAX_TURNS");

        assert_eq!(
            result, 10,
            "recipe settings.max_turns should take priority over env var"
        );
    }

    #[test]
    #[serial]
    fn test_resolve_max_turns_falls_back_to_env_var() {
        let context = create_test_context();
        let client = SummonClient::new(context).unwrap();

        let session = crate::session::Session::default(); // no recipe

        std::env::set_var("GOOSE_SUBAGENT_MAX_TURNS", "7");
        let result = client.resolve_max_turns(&session);
        std::env::remove_var("GOOSE_SUBAGENT_MAX_TURNS");

        assert_eq!(
            result, 7,
            "should fall back to GOOSE_SUBAGENT_MAX_TURNS env var"
        );
    }

    #[test]
    #[serial]
    fn test_resolve_max_turns_falls_back_to_default() {
        let context = create_test_context();
        let client = SummonClient::new(context).unwrap();

        let session = crate::session::Session::default(); // no recipe

        std::env::remove_var("GOOSE_SUBAGENT_MAX_TURNS");
        let result = client.resolve_max_turns(&session);

        assert_eq!(
            result,
            crate::agents::subagent_task_config::DEFAULT_SUBAGENT_MAX_TURNS,
            "should fall back to DEFAULT_SUBAGENT_MAX_TURNS"
        );
    }

    fn empty_recipe() -> crate::recipe::Recipe {
        crate::recipe::Recipe {
            version: "1.0.0".to_string(),
            title: String::new(),
            description: String::new(),
            instructions: None,
            prompt: None,
            extensions: None,
            settings: None,
            activities: None,
            author: None,
            parameters: None,
            response: None,
            sub_recipes: None,
            retry: None,
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_provider_reuses_unregistered_parent_provider() {
        let temp_dir = TempDir::new().unwrap();
        let parent_provider: Arc<dyn crate::providers::base::Provider> = Arc::new(
            crate::providers::testprovider::TestProvider::new_replaying(
                temp_dir.path().join("records.json").display().to_string(),
            )
            .unwrap(),
        );
        let extension_manager = Arc::new(
            crate::agents::extension_manager::ExtensionManager::new_without_provider(
                temp_dir.path().to_path_buf(),
            ),
        );
        *extension_manager.get_provider().lock().await = Some(Arc::clone(&parent_provider));
        let mut context = extension_manager.get_context().clone();
        context.extension_manager = Some(Arc::downgrade(&extension_manager));
        let client = SummonClient::new(context).unwrap();
        let session = crate::session::Session {
            provider_name: Some(parent_provider.get_name().to_string()),
            model_config: Some(goose_providers::model::ModelConfig::new("test-model")),
            ..Default::default()
        };

        let params = DelegateParams {
            provider: Some(parent_provider.get_name().to_string()),
            model: Some("test-model".to_string()),
            ..Default::default()
        };
        let (resolved_provider, _) = client
            .resolve_provider(&params, &empty_recipe(), &session, &[])
            .await
            .unwrap();

        assert!(Arc::ptr_eq(&parent_provider, &resolved_provider));
    }

    #[tokio::test]
    async fn test_build_task_config_recreates_registered_parent_provider() {
        let temp_dir = TempDir::new().unwrap();
        let parent_provider = providers::create("openai", Vec::new()).await.unwrap();
        let extension_manager = Arc::new(
            crate::agents::extension_manager::ExtensionManager::new_without_provider(
                temp_dir.path().to_path_buf(),
            ),
        );
        *extension_manager.get_provider().lock().await = Some(Arc::clone(&parent_provider));
        let mut context = extension_manager.get_context().clone();
        context.extension_manager = Some(Arc::downgrade(&extension_manager));
        let client = SummonClient::new(context).unwrap();
        let session = crate::session::Session {
            provider_name: Some(parent_provider.get_name().to_string()),
            model_config: Some(goose_providers::model::ModelConfig::new("test-model")),
            working_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let params = DelegateParams {
            extensions: Some(Vec::new()),
            provider: Some(parent_provider.get_name().to_string()),
            model: Some("test-model".to_string()),
            ..Default::default()
        };

        let task_config = client
            .build_task_config(&params, &empty_recipe(), &session)
            .await
            .unwrap();

        assert!(!Arc::ptr_eq(&parent_provider, &task_config.provider));
        assert!(task_config.extensions.is_empty());
    }

    const PARENT_MODEL: &str = "claude-3-5-sonnet-20241022";
    const OVERRIDE_MODEL: &str = "claude-opus-4-6";
    const PROVIDER: &str = "anthropic";

    fn session_with(parent: goose_providers::model::ModelConfig) -> crate::session::Session {
        crate::session::Session {
            provider_name: Some(PROVIDER.to_string()),
            model_config: Some(parent),
            ..Default::default()
        }
    }

    fn resolve_with_override(
        model: Option<&str>,
        parent: goose_providers::model::ModelConfig,
    ) -> goose_providers::model::ModelConfig {
        let client = SummonClient::new(create_test_context()).unwrap();
        let params = DelegateParams {
            model: model.map(String::from),
            ..Default::default()
        };
        client
            .resolve_model_config(
                &params,
                &empty_recipe(),
                &session_with(parent),
                PROVIDER,
                None,
            )
            .expect("resolve_model_config")
    }

    fn parent_config() -> goose_providers::model::ModelConfig {
        goose_providers::model::ModelConfig::new(PARENT_MODEL).with_canonical_limits(PROVIDER)
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_applies_canonical_limits_to_overridden_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
        ]);

        let parent = parent_config();
        let overridden = goose_providers::model::ModelConfig::new(OVERRIDE_MODEL)
            .with_canonical_limits(PROVIDER);
        assert_ne!(parent.reasoning, overridden.reasoning);

        let resolved = resolve_with_override(Some(OVERRIDE_MODEL), parent);

        assert_eq!(resolved.model_name, OVERRIDE_MODEL);
        assert_eq!(resolved.max_tokens, overridden.max_tokens);
        assert_eq!(resolved.reasoning, overridden.reasoning);
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_does_not_inherit_provider_specific_request_params() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
        ]);

        // Parent session is a Claude model with anthropic_beta in request_params.
        // When delegate() overrides to a different model (e.g. Gemini), provider-
        // specific params like anthropic_beta must not bleed through — they would
        // cause a 400 INVALID_ARGUMENT from the target API.
        let mut parent = parent_config();
        parent.request_params = Some(HashMap::from([(
            "anthropic_beta".to_string(),
            serde_json::json!("custom-beta-header"),
        )]));

        let resolved = resolve_with_override(Some(OVERRIDE_MODEL), parent);

        assert_eq!(
            resolved
                .request_params
                .as_ref()
                .and_then(|p| p.get("anthropic_beta")),
            None,
            "anthropic_beta must not be inherited by a child session with a different model"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_inherits_thinking_effort_on_override() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
        ]);

        // Reasoning controls are model-family-agnostic and should be inherited,
        // while provider-specific params like anthropic_beta must not.
        let mut parent = parent_config();
        parent.request_params = Some(HashMap::from([
            ("thinking_effort".to_string(), serde_json::json!("high")),
            ("budget_tokens".to_string(), serde_json::json!(8192)),
            (
                "anthropic_beta".to_string(),
                serde_json::json!("custom-beta-header"),
            ),
        ]));

        let resolved = resolve_with_override(Some(OVERRIDE_MODEL), parent);

        assert_eq!(
            resolved
                .request_params
                .as_ref()
                .and_then(|p| p.get("thinking_effort")),
            Some(&serde_json::json!("high")),
            "thinking_effort should be inherited across model families"
        );
        assert_eq!(
            resolved
                .request_params
                .as_ref()
                .and_then(|p| p.get("budget_tokens")),
            Some(&serde_json::json!(8192)),
            "budget_tokens should be inherited across model families"
        );
        assert_eq!(
            resolved
                .request_params
                .as_ref()
                .and_then(|p| p.get("anthropic_beta")),
            None,
            "anthropic_beta must not be inherited alongside reasoning controls"
        );
    }

    fn extract_text(content: &ContentBlock) -> &str {
        use rmcp::model::ContentBlock;
        match content {
            ContentBlock::Text(t) => t.text.as_str(),
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_env_var_overrides_params_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", Some(OVERRIDE_MODEL)),
        ]);

        let client = SummonClient::new(create_test_context()).unwrap();
        let params = DelegateParams {
            model: Some("params-model".to_string()),
            ..Default::default()
        };
        let result = client
            .resolve_model_config(
                &params,
                &empty_recipe(),
                &session_with(parent_config()),
                PROVIDER,
                None,
            )
            .expect("resolve_model_config");
        assert_eq!(
            result.model_name, OVERRIDE_MODEL,
            "GOOSE_SUBAGENT_MODEL must take priority over params.model"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_env_var_overrides_recipe_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", Some(OVERRIDE_MODEL)),
        ]);

        let client = SummonClient::new(create_test_context()).unwrap();
        let mut recipe = empty_recipe();
        recipe.settings = Some(crate::recipe::Settings {
            goose_provider: None,
            goose_model: Some("recipe-model".to_string()),
            temperature: None,
            max_turns: None,
        });
        let result = client
            .resolve_model_config(
                &DelegateParams::default(),
                &recipe,
                &session_with(parent_config()),
                PROVIDER,
                None,
            )
            .expect("resolve_model_config");
        assert_eq!(
            result.model_name, OVERRIDE_MODEL,
            "GOOSE_SUBAGENT_MODEL must take priority over recipe settings"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_env_provider_uses_provider_default_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_PROVIDER", Some(PROVIDER)),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
            ("ANTHROPIC_API_KEY", Some("test-key")),
        ]);

        let client = SummonClient::new(create_test_context()).unwrap();
        let params = DelegateParams {
            provider: Some("openai".to_string()),
            model: Some("model-for-another-provider".to_string()),
            ..Default::default()
        };
        let mut recipe = empty_recipe();
        recipe.settings = Some(crate::recipe::Settings {
            goose_provider: Some("openai".to_string()),
            goose_model: Some("recipe-model-for-another-provider".to_string()),
            temperature: None,
            max_turns: None,
        });
        let default_model = providers::get_from_registry(PROVIDER)
            .await
            .unwrap()
            .metadata()
            .default_model
            .clone();
        let session = crate::session::Session {
            provider_name: Some("openai".to_string()),
            model_config: Some(goose_providers::model::ModelConfig::new(
                "parent-openai-model",
            )),
            ..Default::default()
        };
        let (_, result) = client
            .resolve_provider(&params, &recipe, &session, &[])
            .await
            .expect("resolve_provider");

        assert_eq!(result.model_name, default_model);
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_env_provider_keeps_matching_params_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_PROVIDER", Some(PROVIDER)),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
            ("ANTHROPIC_API_KEY", Some("test-key")),
        ]);

        let client = SummonClient::new(create_test_context()).unwrap();
        let params = DelegateParams {
            provider: Some(PROVIDER.to_string()),
            model: Some(OVERRIDE_MODEL.to_string()),
            ..Default::default()
        };
        let (_, result) = client
            .resolve_provider(
                &params,
                &empty_recipe(),
                &session_with(parent_config()),
                &[],
            )
            .await
            .expect("resolve_provider");

        assert_eq!(result.model_name, OVERRIDE_MODEL);
    }

    #[tokio::test]
    #[serial]
    async fn test_resolve_model_config_dynamic_provider_requires_model() {
        let _env = env_lock::lock_env([
            ("GOOSE_CONTEXT_LIMIT", None::<&str>),
            ("GOOSE_MAX_TOKENS", None::<&str>),
            ("GOOSE_SUBAGENT_MODEL", None::<&str>),
        ]);

        let default_model = providers::get_from_registry("lmstudio")
            .await
            .unwrap()
            .metadata()
            .default_model
            .clone();
        assert!(default_model.is_empty());

        let client = SummonClient::new(create_test_context()).unwrap();
        let params = DelegateParams {
            provider: Some("openai".to_string()),
            model: Some("openai-model".to_string()),
            ..Default::default()
        };
        let session = crate::session::Session {
            provider_name: Some("openai".to_string()),
            model_config: Some(goose_providers::model::ModelConfig::new(
                "parent-openai-model",
            )),
            ..Default::default()
        };
        let error = client
            .resolve_model_config(
                &params,
                &empty_recipe(),
                &session,
                "lmstudio",
                Some(&default_model),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("No model configured for provider 'lmstudio'"));
    }

    fn test_tool_notification(request_id: &str, subagent_id: &str) -> ServerNotification {
        use crate::agents::subagent_handler::create_tool_notification;
        use crate::conversation::message::MessageContent;
        use rmcp::model::CallToolRequestParams;

        let tool_call = CallToolRequestParams::new("developer__shell").with_arguments(
            serde_json::json!({"command": request_id})
                .as_object()
                .unwrap()
                .clone(),
        );
        let content = MessageContent::tool_request(request_id, Ok(tool_call));
        create_tool_notification(&content, subagent_id).unwrap()
    }

    fn notification_subagent_id(notification: &ServerNotification) -> Option<String> {
        let ServerNotification::LoggingMessageNotification(log) = notification else {
            return None;
        };
        serde_json::to_value(&log.params)
            .ok()?
            .get("data")?
            .get("subagent_id")?
            .as_str()
            .map(str::to_string)
    }

    fn notification_command(notification: &ServerNotification) -> Option<String> {
        let ServerNotification::LoggingMessageNotification(log) = notification else {
            return None;
        };
        serde_json::to_value(&log.params)
            .ok()?
            .get("data")?
            .get("tool_call")?
            .get("arguments")?
            .get("command")?
            .as_str()
            .map(str::to_string)
    }

    fn notification_channel() -> (
        ToolCallNotificationEmitter,
        tokio::sync::mpsc::Receiver<ServerNotification>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        (ToolCallNotificationEmitter::new(sender), receiver)
    }

    fn buffered_notification_sink(
        notifications: Vec<ServerNotification>,
    ) -> SharedNotificationSink {
        Arc::new(Mutex::new(NotificationSink::Buffer(notifications)))
    }

    #[test]
    fn test_is_session_id() {
        assert!(is_session_id("20260204_1"));
        assert!(is_session_id("20260204_42"));
        assert!(is_session_id("20260204_999"));
        assert!(!is_session_id("task_12345_0001"));
        assert!(!is_session_id("my-recipe"));
        assert!(!is_session_id("2026020_1"));
        assert!(!is_session_id("20260204"));
    }

    #[tokio::test]
    async fn test_notification_sinks_isolate_concurrent_delegate_calls() {
        let (emitter_a, mut notifications_a) = notification_channel();
        let (emitter_b, mut notifications_b) = notification_channel();
        let sink_a = SummonClient::notification_sink(Some(emitter_a));
        let sink_b = SummonClient::notification_sink(Some(emitter_b));

        let (result_a, result_b) = tokio::join!(
            SummonClient::run_subagent_with_notifications(sink_a, |notification_tx| async move {
                notification_tx
                    .send(test_tool_notification("inner-a", "subagent-a"))
                    .unwrap();
                tokio::task::yield_now().await;
                Ok("delegate-a".to_string())
            }),
            SummonClient::run_subagent_with_notifications(sink_b, |notification_tx| async move {
                notification_tx
                    .send(test_tool_notification("inner-b", "subagent-b"))
                    .unwrap();
                tokio::task::yield_now().await;
                Ok("delegate-b".to_string())
            })
        );
        assert_eq!(result_a.unwrap(), "delegate-a");
        assert_eq!(result_b.unwrap(), "delegate-b");

        let notification_a = notifications_a.recv().await.unwrap();
        let notification_b = notifications_b.recv().await.unwrap();
        assert_eq!(
            notification_subagent_id(&notification_a).as_deref(),
            Some("subagent-a")
        );
        assert_eq!(
            notification_subagent_id(&notification_b).as_deref(),
            Some("subagent-b")
        );
        assert!(notifications_a.try_recv().is_err());
        assert!(notifications_b.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_live_notifications_precede_delegate_result() {
        use crate::agents::tool_execution::{tool_stream, ToolStreamItem};
        use tokio_stream::wrappers::ReceiverStream;

        for _ in 0..32 {
            let (emitter, notifications) = notification_channel();
            let sink = SummonClient::notification_sink(Some(emitter));
            let mut output = tool_stream(
                ReceiverStream::new(notifications),
                futures::stream::empty(),
                async move {
                    let result = SummonClient::run_subagent_with_notifications(
                        sink,
                        |notification_tx| async move {
                            for command in ["inner-live-0", "inner-live-1", "inner-live-2"] {
                                notification_tx
                                    .send(test_tool_notification(command, "subagent-live"))
                                    .unwrap();
                            }
                            Ok("delegate-result".to_string())
                        },
                    )
                    .await
                    .unwrap();
                    Ok::<_, rmcp::model::ErrorData>(CallToolResult::success(vec![
                        ContentBlock::text(result),
                    ]))
                },
            );

            let mut commands = Vec::new();
            let result = loop {
                match output.next().await.unwrap() {
                    ToolStreamItem::Message(notification) => {
                        assert_eq!(
                            notification_subagent_id(&notification).as_deref(),
                            Some("subagent-live")
                        );
                        commands.push(notification_command(&notification).unwrap());
                    }
                    ToolStreamItem::Result(result) => break result,
                    ToolStreamItem::ActionRequired(_) => {
                        panic!("delegate must not request an action")
                    }
                }
            };

            assert_eq!(commands, ["inner-live-0", "inner-live-1", "inner-live-2"]);
            assert!(result.is_ok());
            assert!(output.next().await.is_none());
        }
    }

    #[tokio::test]
    async fn test_async_completion_before_load_replays_notifications() {
        use crate::agents::tool_execution::{tool_stream, ToolStreamItem};
        use tokio_stream::wrappers::ReceiverStream;

        let client = Arc::new(SummonClient::new(create_test_context()).unwrap());
        let task_id = "20260204_1";
        let buffered = vec![test_tool_notification("inner-completed", task_id)];
        client.completed_tasks.lock().await.insert(
            task_id.to_string(),
            CompletedTask {
                id: task_id.to_string(),
                description: "Completed task".to_string(),
                result: Ok("done".to_string()),
                turns_taken: 1,
                duration: Duration::from_secs(1),
                completed_at: Instant::now(),
                notification_sink: buffered_notification_sink(buffered),
            },
        );
        let (emitter, notifications) = notification_channel();
        let load_client = Arc::clone(&client);
        let mut output = tool_stream(
            ReceiverStream::new(notifications),
            futures::stream::empty(),
            async move {
                let result = load_client
                    .handle_load_task_result(task_id, false, false, Some(emitter))
                    .await
                    .unwrap();
                Ok::<_, rmcp::model::ErrorData>(CallToolResult::success(result.content))
            },
        );

        let ToolStreamItem::Message(notification) = output.next().await.unwrap() else {
            panic!("buffered notification must be emitted before the load result");
        };
        assert_eq!(
            notification_subagent_id(&notification).as_deref(),
            Some(task_id)
        );
        assert_eq!(
            notification_command(&notification).as_deref(),
            Some("inner-completed")
        );
        let ToolStreamItem::Result(result) = output.next().await.unwrap() else {
            panic!("load result must follow buffered notifications");
        };
        assert!(result.is_ok());
        assert!(output.next().await.is_none());
        assert!(!client.completed_tasks.lock().await.contains_key(task_id));
    }

    #[tokio::test]
    async fn test_cancelled_completed_load_remains_retrievable() {
        let client = Arc::new(SummonClient::new(create_test_context()).unwrap());
        let task_id = "20260204_1";
        client.completed_tasks.lock().await.insert(
            task_id.to_string(),
            CompletedTask {
                id: task_id.to_string(),
                description: "Completed task".to_string(),
                result: Ok("done".to_string()),
                turns_taken: 1,
                duration: Duration::from_secs(1),
                completed_at: Instant::now(),
                notification_sink: buffered_notification_sink(vec![
                    test_tool_notification("inner-0", task_id),
                    test_tool_notification("inner-1", task_id),
                ]),
            },
        );
        let (emitter, mut notifications) = notification_channel();
        let load_client = Arc::clone(&client);
        let load = tokio::spawn(async move {
            load_client
                .handle_load_task_result(task_id, false, false, Some(emitter))
                .await
        });

        let first = notifications.recv().await.unwrap();
        assert_eq!(notification_command(&first).as_deref(), Some("inner-0"));
        load.abort();
        assert!(load.await.unwrap_err().is_cancelled());
        assert!(client.completed_tasks.lock().await.contains_key(task_id));

        let (retry_emitter, mut retry_notifications) = notification_channel();
        let result = client
            .handle_load_task_result(task_id, false, false, Some(retry_emitter))
            .await
            .unwrap();

        assert_eq!(result.status, "completed");
        for command in ["inner-0", "inner-1"] {
            let notification = retry_notifications.try_recv().unwrap();
            assert_eq!(
                notification_command(&notification).as_deref(),
                Some(command)
            );
        }
        assert!(retry_notifications.try_recv().is_err());
        assert!(!client.completed_tasks.lock().await.contains_key(task_id));
    }

    #[tokio::test]
    async fn test_buffered_replay_preserves_order_and_emitter_capacity() {
        let sink = buffered_notification_sink(
            (0..33)
                .map(|index| test_tool_notification(&format!("inner-{index}"), "subagent"))
                .collect(),
        );
        let (emitter, mut notifications) = notification_channel();

        SummonClient::attach_notification_emitter(&sink, Some(emitter)).await;

        for index in 0..32 {
            let notification = notifications.try_recv().unwrap();
            assert_eq!(
                notification_command(&notification),
                Some(format!("inner-{index}"))
            );
        }
        assert!(notifications.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_load_completes_when_caller_does_not_consume_notifications() {
        let client = SummonClient::new(create_test_context()).unwrap();
        let task_id = "20260204_1";
        client.completed_tasks.lock().await.insert(
            task_id.to_string(),
            CompletedTask {
                id: task_id.to_string(),
                description: "Completed task".to_string(),
                result: Ok("done".to_string()),
                turns_taken: 1,
                duration: Duration::from_secs(1),
                completed_at: Instant::now(),
                notification_sink: buffered_notification_sink(
                    (0..64)
                        .map(|index| test_tool_notification(&format!("inner-{index}"), task_id))
                        .collect(),
                ),
            },
        );
        let (emitter, _notifications) = notification_channel();

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            client.handle_load_task_result(task_id, false, false, Some(emitter)),
        )
        .await
        .expect("load must not wait for a notification consumer")
        .unwrap();

        assert_eq!(result.status, "completed");
    }

    #[tokio::test]
    async fn test_async_task_result_lifecycle() {
        let client = SummonClient::new(create_test_context()).unwrap();
        let temp_dir = TempDir::new().unwrap();

        let result = client
            .handle_load_task_result("20260204_999", false, false, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));

        {
            let notification_sink =
                buffered_notification_sink(vec![test_tool_notification("req1", "20260204_1")]);
            let (handle, completion_token) = spawn_background_task(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok("done".to_string())
            });

            let mut running = client.background_tasks.lock().await;
            running.insert(
                "20260204_1".to_string(),
                BackgroundTask {
                    id: "20260204_1".to_string(),
                    description: "Running task".to_string(),
                    started_at: Instant::now(),
                    turns: Arc::new(AtomicU32::new(2)),
                    last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                    handle,
                    cancellation_token: CancellationToken::new(),
                    completion_token,
                    notification_sink,
                },
            );
        }

        let (emitter, mut notifications) = notification_channel();
        let (result, notification) = tokio::join!(
            client.handle_load_task_result("20260204_1", false, false, Some(emitter)),
            notifications.recv()
        );
        let result = result.expect("load should wait and return result");
        let text = extract_text(&result.content[0]);
        assert!(text.contains("Completed"));
        assert!(text.contains("done"));

        let notif = notification.expect("load emitter should receive buffered notification");
        if let ServerNotification::LoggingMessageNotification(log) = notif {
            let params = serde_json::to_value(&log.params).unwrap();
            let data = params.get("data").and_then(|v| v.as_object()).unwrap();
            assert_eq!(
                data.get("subagent_id").and_then(|v| v.as_str()),
                Some("20260204_1")
            );
        } else {
            panic!("expected logging notification");
        }

        {
            let mut completed = client.completed_tasks.lock().await;
            completed.insert(
                "20260204_2".to_string(),
                CompletedTask {
                    id: "20260204_2".to_string(),
                    description: "Successful task".to_string(),
                    result: Ok("Task completed successfully with output".to_string()),
                    turns_taken: 5,
                    duration: Duration::from_secs(60),
                    completed_at: Instant::now(),
                    notification_sink: buffered_notification_sink(Vec::new()),
                },
            );
            completed.insert(
                "20260204_3".to_string(),
                CompletedTask {
                    id: "20260204_3".to_string(),
                    description: "Failed task".to_string(),
                    result: Err("Something went wrong".to_string()),
                    turns_taken: 3,
                    duration: Duration::from_secs(30),
                    completed_at: Instant::now(),
                    notification_sink: buffered_notification_sink(Vec::new()),
                },
            );
        }

        let moim = client.get_moim("test").await.unwrap();
        assert!(moim.contains("20260204_2"));
        assert!(moim.contains("20260204_3"));
        assert!(moim.contains(r#"use load("20260204_2") to get result"#));
        assert!(moim.contains(r#"use load("20260204_3") to get result"#));

        let discovery = client
            .handle_load_discovery("test", temp_dir.path())
            .await
            .unwrap();
        let discovery_text = extract_text(&discovery[0]);
        assert!(discovery_text.contains("Completed Tasks (awaiting retrieval)"));
        assert!(discovery_text.contains("20260204_2"));
        assert!(discovery_text.contains("20260204_3"));

        let result = client
            .handle_load_task_result("20260204_2", false, false, None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        assert!(text.contains("20260204_2"));
        assert!(text.contains("Successful task"));
        assert!(text.contains("✓ Completed"));
        assert!(text.contains("1m"));
        assert!(text.contains("5 turns"));
        assert!(text.contains("Task completed successfully with output"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.turns, Some(5));

        assert!(!client
            .completed_tasks
            .lock()
            .await
            .contains_key("20260204_2"));

        let result = client
            .handle_load_task_result("20260204_3", false, false, None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        assert!(text.contains("✗ Failed"));
        assert!(text.contains("Error: Something went wrong"));
        assert_eq!(result.status, "failed");

        let result = client
            .handle_load_task_result("20260204_3", false, false, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));

        // All tasks consumed -- moim should be empty
        assert!(client.get_moim("test").await.is_none());
    }

    #[tokio::test]
    async fn test_completed_task_uses_durable_tool_turns_in_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(crate::session::SessionManager::new(
            temp_dir.path().join("sessions"),
        ));
        let task_id = create_test_subagent_session(
            &session_manager,
            temp_dir.path(),
            &[
                Message::user().with_text("Inspect the project"),
                Message::assistant().with_tool_request(
                    "tool-1",
                    Ok(rmcp::model::CallToolRequestParams::new("test_tool")),
                ),
                Message::user().with_tool_response(
                    "tool-1",
                    Ok(CallToolResult::success(vec![ContentBlock::text("done")])),
                ),
                Message::assistant().with_text("Inspection complete"),
            ],
        )
        .await;
        let client =
            SummonClient::new(create_test_context_with_session_manager(session_manager)).unwrap();

        let handle = tokio::spawn(async { Ok("Inspection complete".to_string()) });
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
        client.background_tasks.lock().await.insert(
            task_id.clone(),
            BackgroundTask {
                id: task_id.clone(),
                description: "Inspect the project".to_string(),
                started_at: Instant::now(),
                // Simulate hundreds of streamed message events for two durable turns.
                turns: Arc::new(AtomicU32::new(554)),
                last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                handle,
                cancellation_token: CancellationToken::new(),
                completion_token: CancellationToken::new(),
                notification_sink: buffered_notification_sink(Vec::new()),
            },
        );

        let arguments = serde_json::json!({"source": task_id})
            .as_object()
            .unwrap()
            .clone();
        let result = client
            .handle_load("parent", Some(arguments), None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        let meta = result.meta.unwrap();

        assert!(text.contains("(2 turns)"));
        assert_eq!(meta.0.get("turns_taken"), Some(&serde_json::json!(2)));
        assert_eq!(
            meta.0.get("task_status"),
            Some(&serde_json::json!("completed"))
        );
    }

    #[test]
    fn test_durable_turn_count_ignores_compaction_scaffolding() {
        let compacted_tool_loop = crate::conversation::Conversation::new_unvalidated(vec![
            Message::user()
                .with_text("Previous task")
                .with_visibility(true, false),
            Message::assistant()
                .with_text("Previous result")
                .with_visibility(true, false),
            Message::user()
                .with_text("Inspect the project")
                .with_visibility(true, false),
            Message::assistant()
                .with_tool_request(
                    "tool-1",
                    Ok(rmcp::model::CallToolRequestParams::new("test_tool")),
                )
                .with_visibility(true, false),
            Message::user()
                .with_tool_response(
                    "tool-1",
                    Ok(CallToolResult::success(vec![ContentBlock::text("done")])),
                )
                .with_visibility(true, false),
            // A later compaction archives its earlier agent-only scaffold.
            Message::assistant()
                .with_text("<older summary>")
                .with_visibility(false, false),
            Message::user()
                .with_text("Inspect the project")
                .with_visibility(false, false),
            Message::assistant()
                .with_text("<summary of earlier work>")
                .with_text("Continue from the compacted context")
                .agent_only(),
            Message::user()
                .with_text("Inspect the project")
                .agent_only(),
            Message::assistant().with_text("Inspection complete"),
        ]);
        assert_eq!(durable_assistant_turn_count(&compacted_tool_loop), 2);

        // Keep the hidden projected user message as a role boundary. Dropping
        // every agent-only message would merge the failed and retried replies.
        let compacted_retry = crate::conversation::Conversation::new_unvalidated(vec![
            Message::user()
                .with_text("Inspect the project")
                .with_visibility(true, false),
            Message::assistant()
                .with_text("Context window exceeded")
                .with_visibility(true, false),
            Message::assistant().with_text("<summary>").agent_only(),
            Message::user()
                .with_text("Inspect the project")
                .agent_only(),
            Message::assistant().with_text("Inspection complete"),
        ]);
        assert_eq!(durable_assistant_turn_count(&compacted_retry), 2);
    }

    #[tokio::test]
    async fn test_cancel_running_task() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(crate::session::SessionManager::new(
            temp_dir.path().join("sessions"),
        ));
        let task_id = create_test_subagent_session(
            &session_manager,
            temp_dir.path(),
            &[Message::user().with_text("Analyse the project")],
        )
        .await;
        let task_session_manager = Arc::clone(&session_manager);
        let client =
            SummonClient::new(create_test_context_with_session_manager(session_manager)).unwrap();
        let token = CancellationToken::new();
        let notification_sink = buffered_notification_sink(Vec::new());
        let task_notification_sink = Arc::clone(&notification_sink);
        let task_token = token.clone();
        let task_notification_id = task_id.clone();

        {
            let mut running = client.background_tasks.lock().await;
            running.insert(
                task_id.clone(),
                BackgroundTask {
                    id: task_id.clone(),
                    description: "Cancellable task".to_string(),
                    started_at: Instant::now(),
                    // This stale event count must be replaced after cancellation.
                    turns: Arc::new(AtomicU32::new(3)),
                    last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                    handle: tokio::spawn(async move {
                        task_token.cancelled().await;
                        task_session_manager
                            .add_message(
                                &task_notification_id,
                                &Message::assistant().with_text("Partial result"),
                            )
                            .await
                            .unwrap();
                        task_notification_sink
                            .lock()
                            .await
                            .route(test_tool_notification("cancel", &task_notification_id));
                        Ok("cancelled gracefully".to_string())
                    }),
                    cancellation_token: token.clone(),
                    completion_token: CancellationToken::new(),
                    notification_sink,
                },
            );
        }

        let (emitter, mut notifications) = notification_channel();
        let (result, notification) = tokio::join!(
            client.handle_load_task_result(&task_id, true, false, Some(emitter)),
            notifications.recv()
        );
        let result = result.unwrap();
        let text = extract_text(&result.content[0]);
        assert!(text.contains("Cancelled"));
        assert!(text.contains(&task_id));
        assert!(text.contains("Cancellable task"));
        assert!(text.contains("cancelled gracefully"));
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.turns, Some(1));
        assert_eq!(
            notification_subagent_id(&notification.unwrap()).as_deref(),
            Some(task_id.as_str())
        );
        assert!(token.is_cancelled());
        assert!(!client.background_tasks.lock().await.contains_key(&task_id));
    }

    #[tokio::test]
    async fn test_cancelled_running_load_remains_retrievable() {
        let client = Arc::new(SummonClient::new(create_test_context()).unwrap());
        let token = CancellationToken::new();
        let task_id = "20260204_1";
        let task_token = token.clone();

        client.background_tasks.lock().await.insert(
            task_id.to_string(),
            BackgroundTask {
                id: task_id.to_string(),
                description: "Cancellable task".to_string(),
                started_at: Instant::now(),
                turns: Arc::new(AtomicU32::new(1)),
                last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                handle: tokio::spawn(async move {
                    task_token.cancelled().await;
                    Ok("cancelled gracefully".to_string())
                }),
                cancellation_token: token.clone(),
                completion_token: CancellationToken::new(),
                notification_sink: buffered_notification_sink(vec![
                    test_tool_notification("inner-0", task_id),
                    test_tool_notification("inner-1", task_id),
                ]),
            },
        );

        let (emitter, mut notifications) = notification_channel();
        let load_client = Arc::clone(&client);
        let load = tokio::spawn(async move {
            load_client
                .handle_load_task_result(task_id, true, false, Some(emitter))
                .await
        });

        let first = notifications.recv().await.unwrap();
        assert_eq!(notification_command(&first).as_deref(), Some("inner-0"));
        load.abort();
        assert!(load.await.unwrap_err().is_cancelled());
        assert!(client.background_tasks.lock().await.contains_key(task_id));
        assert!(!token.is_cancelled());

        let (retry_emitter, mut retry_notifications) = notification_channel();
        let result = client
            .handle_load_task_result(task_id, true, false, Some(retry_emitter))
            .await
            .unwrap();

        assert_eq!(result.status, "cancelled");
        assert!(token.is_cancelled());
        assert!(!client.background_tasks.lock().await.contains_key(task_id));

        let commands: Vec<String> = std::iter::from_fn(|| retry_notifications.try_recv().ok())
            .filter_map(|notification| notification_command(&notification))
            .collect();
        assert!(
            commands == ["inner-0", "inner-1"] || commands == ["inner-1"],
            "retry must replay the remaining notifications, with at-least-once delivery allowed"
        );
    }

    #[tokio::test]
    async fn test_cancelled_waiting_load_remains_retrievable() {
        let client = Arc::new(SummonClient::new(create_test_context()).unwrap());
        let task_id = "20260204_1";
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let (handle, completion_token) = spawn_background_task(async move {
            finish_rx.await.unwrap();
            Ok("done".to_string())
        });

        client.background_tasks.lock().await.insert(
            task_id.to_string(),
            BackgroundTask {
                id: task_id.to_string(),
                description: "Running task".to_string(),
                started_at: Instant::now(),
                turns: Arc::new(AtomicU32::new(1)),
                last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                handle,
                cancellation_token: CancellationToken::new(),
                completion_token,
                notification_sink: buffered_notification_sink(vec![
                    test_tool_notification("inner-0", task_id),
                    test_tool_notification("inner-1", task_id),
                ]),
            },
        );

        let (emitter, mut notifications) = notification_channel();
        let load_client = Arc::clone(&client);
        let load = tokio::spawn(async move {
            load_client
                .handle_load_task_result(task_id, false, false, Some(emitter))
                .await
        });

        let first = notifications.recv().await.unwrap();
        assert_eq!(notification_command(&first).as_deref(), Some("inner-0"));
        load.abort();
        assert!(load.await.unwrap_err().is_cancelled());
        assert!(client.background_tasks.lock().await.contains_key(task_id));

        finish_tx.send(()).unwrap();
        let (retry_emitter, mut retry_notifications) = notification_channel();
        let result = client
            .handle_load_task_result(task_id, false, false, Some(retry_emitter))
            .await
            .unwrap();

        assert_eq!(result.status, "completed");
        assert!(!client.background_tasks.lock().await.contains_key(task_id));

        let commands: Vec<String> = std::iter::from_fn(|| retry_notifications.try_recv().ok())
            .filter_map(|notification| notification_command(&notification))
            .collect();
        assert!(
            commands == ["inner-0", "inner-1"] || commands == ["inner-1"],
            "retry must replay the remaining notifications, with at-least-once delivery allowed"
        );
    }

    #[tokio::test]
    async fn test_dropped_waiting_load_during_turn_refresh_remains_retrievable() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(crate::session::SessionManager::new(
            temp_dir.path().join("sessions"),
        ));
        let task_id = create_test_subagent_session(
            &session_manager,
            temp_dir.path(),
            &[
                Message::user().with_text("Analyse the project"),
                Message::assistant().with_text("done"),
            ],
        )
        .await;
        let client = SummonClient::new(create_test_context_with_session_manager(Arc::clone(
            &session_manager,
        )))
        .unwrap();

        let (handle, completion_token) = spawn_background_task(async { Ok("done".to_string()) });
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
        client.background_tasks.lock().await.insert(
            task_id.clone(),
            BackgroundTask {
                id: task_id.clone(),
                description: "Finished task".to_string(),
                started_at: Instant::now(),
                turns: Arc::new(AtomicU32::new(1)),
                last_activity: Arc::new(AtomicU64::new(current_epoch_millis())),
                handle,
                cancellation_token: CancellationToken::new(),
                completion_token,
                notification_sink: buffered_notification_sink(Vec::new()),
            },
        );

        let pool = session_manager.storage().pool().await.unwrap().clone();
        let mut held_connections = Vec::new();
        for _ in 0..pool.options().get_max_connections() {
            held_connections.push(pool.acquire().await.unwrap());
        }
        tokio::task::yield_now().await;

        let mut load = Box::pin(client.handle_load_task_result(&task_id, false, false, None));
        let first_poll = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(load.as_mut(), cx))
        })
        .await;
        assert!(first_poll.is_pending());
        drop(load);

        assert!(
            client.completed_tasks.lock().await.contains_key(&task_id)
                || client.background_tasks.lock().await.contains_key(&task_id),
            "cancelling a load must not orphan a completed task"
        );

        drop(held_connections);
        let result = client
            .handle_load_task_result(&task_id, false, false, None)
            .await
            .unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.turns, Some(1));
        assert!(extract_text(&result.content[0]).contains("done"));
        assert!(!client.completed_tasks.lock().await.contains_key(&task_id));
        assert!(!client.background_tasks.lock().await.contains_key(&task_id));
    }

    #[tokio::test]
    async fn test_peek_running_task() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(crate::session::SessionManager::new(
            temp_dir.path().join("sessions"),
        ));
        let task_id = create_test_subagent_session(
            &session_manager,
            temp_dir.path(),
            &[Message::user().with_text("Analyse the project")],
        )
        .await;
        let client = SummonClient::new(create_test_context_with_session_manager(Arc::clone(
            &session_manager,
        )))
        .unwrap();
        let last_activity = Arc::new(AtomicU64::new(0));

        {
            let mut running = client.background_tasks.lock().await;
            running.insert(
                task_id.clone(),
                BackgroundTask {
                    id: task_id.clone(),
                    description: "Long running analysis".to_string(),
                    started_at: Instant::now(),
                    // Simulate the old stream-event counter after seven fragments.
                    turns: Arc::new(AtomicU32::new(7)),
                    last_activity: Arc::clone(&last_activity),
                    handle: tokio::spawn(async {
                        tokio::time::sleep(Duration::from_secs(1000)).await;
                        Ok("eventual result".to_string())
                    }),
                    cancellation_token: CancellationToken::new(),
                    completion_token: CancellationToken::new(),
                    notification_sink: buffered_notification_sink(Vec::new()),
                },
            );
        }

        let result = client
            .handle_load_task_result(&task_id, false, true, None)
            .await
            .unwrap();
        assert!(extract_text(&result.content[0]).contains("Task is initialising"));

        // Activity can arrive before the assistant block is durably persisted.
        last_activity.store(current_epoch_millis(), Ordering::Relaxed);
        let result = client
            .handle_load_task_result(&task_id, false, true, None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        assert_eq!(result.turns, Some(0));
        assert!(!text.contains("Task is initialising"));

        for index in 0..7 {
            session_manager
                .add_message(
                    &task_id,
                    &Message::assistant().with_text(format!("fragment {index}")),
                )
                .await
                .unwrap();
        }

        // Peek should return status without removing the task
        let result = client
            .handle_load_task_result(&task_id, false, true, None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        assert!(text.contains("Running"));
        assert!(text.contains("Long running analysis"));
        assert!(text.contains("**Turns taken:** 1"));
        assert_eq!(result.turns, Some(1));

        let moim = client.get_moim("test").await.unwrap();
        assert!(moim.contains("1 turns"));

        // Task should still be in background_tasks (not consumed)
        assert!(client.background_tasks.lock().await.contains_key(&task_id));
    }

    #[tokio::test]
    async fn test_peek_nonexistent_task() {
        let client = SummonClient::new(create_test_context()).unwrap();

        let result = client
            .handle_load_task_result("20260204_999", false, true, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_peek_completed_task_returns_result() {
        let client = SummonClient::new(create_test_context()).unwrap();

        {
            let mut completed = client.completed_tasks.lock().await;
            completed.insert(
                "20260204_1".to_string(),
                CompletedTask {
                    id: "20260204_1".to_string(),
                    description: "Finished task".to_string(),
                    result: Ok("final output".to_string()),
                    turns_taken: 4,
                    duration: Duration::from_secs(30),
                    completed_at: Instant::now(),
                    notification_sink: buffered_notification_sink(Vec::new()),
                },
            );
        }

        // Peek on a completed task should return the full result (same as non-peek)
        let result = client
            .handle_load_task_result("20260204_1", false, true, None)
            .await
            .unwrap();
        let text = extract_text(&result.content[0]);
        assert!(text.contains("Completed"));
        assert!(text.contains("final output"));

        // Peek must be non-destructive: the result is still retrievable afterwards.
        assert!(client
            .completed_tasks
            .lock()
            .await
            .contains_key("20260204_1"));
        let result = client
            .handle_load_task_result("20260204_1", false, false, None)
            .await
            .unwrap();
        assert!(extract_text(&result.content[0]).contains("final output"));
    }
}
