use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell as ClapShell};
use clap_complete_nushell::Nushell as ClapNushell;
use goose::agents::GoosePlatform;
use goose::builtin_extension::register_builtin_extensions;
use goose::config::{Config, GooseMode};
#[cfg(feature = "telemetry")]
use goose::posthog::get_telemetry_choice;
use goose::recipe::Recipe;
use goose::source_roots::SourceRoot;
use goose_mcp::mcp_server_runner::{serve, McpCommand};
use goose_mcp::{AutoVisualiserRouter, ComputerControllerServer, MemoryServer, TutorialServer};

#[cfg(feature = "telemetry")]
use crate::commands::configure::configure_telemetry_consent_dialog;
use crate::commands::configure::handle_configure;
use crate::commands::info::handle_info;
use crate::commands::plugin::{handle_plugin_install, handle_plugin_update};
use crate::commands::recipe::{handle_deeplink, handle_list, handle_open, handle_validate};
#[cfg(feature = "roaming")]
use crate::commands::roam::{handle_roam_command, RoamCommand};
use crate::commands::term::{
    handle_term_info, handle_term_init, handle_term_log, handle_term_run, Shell,
};

use crate::commands::schedule::{
    handle_schedule_add, handle_schedule_cron_help, handle_schedule_list, handle_schedule_remove,
    handle_schedule_run_now, handle_schedule_services_status, handle_schedule_services_stop,
    handle_schedule_sessions,
};
use crate::commands::session::{handle_session_list, handle_session_remove};
use crate::commands::skills::handle_skills_list;
use crate::recipes::extract_from_cli::extract_recipe_info_from_cli;
use crate::recipes::recipe::{explain_recipe, render_recipe_as_yaml};
use crate::session::{build_session, SessionBuilderConfig};
use goose::agents::Container;
use goose::session::session_manager::SessionType;
use goose::session::SessionManager;
use std::io::Read;
use std::path::PathBuf;
const GOOSE_SERVER_SECRET_KEY_ENV: &str = "GOOSE_SERVER__SECRET_KEY";

fn generate_serve_secret_key() -> String {
    use rand::distr::{Alphanumeric, SampleString};

    format!(
        "goose-acp-{}",
        Alphanumeric.sample_string(&mut rand::rng(), 32)
    )
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ServePlatform {
    #[default]
    Cli,
    Desktop,
}

impl From<ServePlatform> for GoosePlatform {
    fn from(platform: ServePlatform) -> Self {
        match platform {
            ServePlatform::Cli => GoosePlatform::GooseCli,
            ServePlatform::Desktop => GoosePlatform::GooseDesktop,
        }
    }
}

#[derive(Parser)]
#[command(name = "goose", author, version, display_name = "", about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args, Debug, Clone)]
#[group(required = false, multiple = false)]
pub struct Identifier {
    #[arg(
        short = 'n',
        long,
        value_name = "NAME",
        help = "Name for the chat session (e.g., 'project-x')",
        long_help = "Specify a name for your chat session. When used with --resume, will resume this specific session if it exists."
    )]
    pub name: Option<String>,

    #[arg(
        long = "session-id",
        alias = "id",
        value_name = "SESSION_ID",
        help = "Session ID (e.g., '20250921_143022')",
        long_help = "Specify a session ID to resume. Requires --resume."
    )]
    pub session_id: Option<String>,

    #[arg(
        long,
        value_name = "PATH",
        help = "Legacy: Path for the chat session",
        long_help = "Legacy parameter for backward compatibility. Extracts session ID from the file path (e.g., '/path/to/20250325_200615.
jsonl' -> '20250325_200615')."
    )]
    pub path: Option<PathBuf>,
}

/// Session behavior options shared between Session and Run commands
#[derive(Args, Debug, Clone, Default)]
pub struct SessionOptions {
    #[arg(
        long,
        help = "Enable debug output mode with full content and no truncation",
        long_help = "When enabled, shows complete tool responses without truncation and full paths."
    )]
    pub debug: bool,

    #[arg(
        long = "max-tool-repetitions",
        value_name = "NUMBER",
        help = "Maximum number of consecutive identical tool calls allowed",
        long_help = "Set a limit on how many times the same tool can be called consecutively with identical parameters. Helps prevent infinite loops."
    )]
    pub max_tool_repetitions: Option<u32>,

    #[arg(
        long = "max-turns",
        value_name = "NUMBER",
        help = "Maximum number of turns allowed without user input (default: 1000)",
        long_help = "Set a limit on how many turns (iterations) the agent can take without asking for user input to continue."
    )]
    pub max_turns: Option<u32>,

    #[arg(
        long = "container",
        value_name = "CONTAINER_ID",
        help = "Docker container ID to run extensions inside",
        long_help = "Run extensions (stdio and built-in) inside the specified container. The extension must exist in the container. For built-in extensions, goose must be installed inside the container."
    )]
    pub container: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamableHttpOptions {
    pub url: String,
    pub timeout: u64,
}

fn parse_streamable_http_extension(input: &str) -> Result<StreamableHttpOptions, String> {
    let mut input_iter = input.split_whitespace();
    let (mut url, mut timeout) = (String::new(), goose::config::DEFAULT_EXTENSION_TIMEOUT);

    if let Some(url_str) = input_iter.next() {
        url.push_str(url_str);
    }

    for kv_pair in input_iter {
        if !kv_pair.contains('=') {
            continue;
        }

        let (key, value) = kv_pair.split_once('=').unwrap();

        // We Can have more keys here for setting other properties
        if key == "timeout" {
            if let Ok(seconds) = value.parse::<u64>() {
                timeout = seconds;
            }
        }
    }

    Ok(StreamableHttpOptions { url, timeout })
}

/// Extension configuration options shared between Session and Run commands
#[derive(Args, Debug, Clone, Default)]
pub struct ExtensionOptions {
    #[arg(
        long = "with-extension",
        value_name = "COMMAND",
        help = "Add stdio extensions (can be specified multiple times)",
        long_help = "Add stdio extensions from full commands with environment variables. Can be specified multiple times. Format: '[name:]ENV1=val1 ENV2=val2 command args...'. Without the optional name, the extension is named after the command, which is the launcher for anything started through one ('npx', 'python', 'uvx', ...); extensions that would end up sharing a name are instead named after their full command line.",
        action = clap::ArgAction::Append
    )]
    pub extensions: Vec<String>,

    #[arg(
        long = "with-streamable-http-extension",
        value_name = "URL",
        help = "Add streamable HTTP extensions (can be specified multiple times)",
        long_help = "Add streamable HTTP extensions from a URL. Can be specified multiple times. Format: 'url...' or 'url... timeout=100' to set up timeout other than default",
        action = clap::ArgAction::Append,
        value_parser = parse_streamable_http_extension
    )]
    pub streamable_http_extensions: Vec<StreamableHttpOptions>,

    #[arg(
        long = "with-builtin",
        value_name = "NAME",
        help = "Add builtin extensions by name (e.g., 'developer' or multiple: 'developer,github')",
        long_help = "Add one or more builtin extensions that are bundled with goose by specifying their names, comma-separated",
        value_delimiter = ','
    )]
    pub builtins: Vec<String>,

    #[arg(
        long = "no-profile",
        help = "Don't load your default extensions, only use CLI-specified extensions"
    )]
    pub no_profile: bool,
}

/// Input source and recipe options for the run command
#[derive(Args, Debug, Clone, Default)]
pub struct InputOptions {
    /// Path to instruction file containing commands
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Path to instruction file containing commands. Use - for stdin.",
        conflicts_with = "input_text",
        conflicts_with = "recipe"
    )]
    pub instructions: Option<String>,

    /// Input text containing commands
    #[arg(
        short = 't',
        long = "text",
        value_name = "TEXT",
        help = "Input text to provide to goose directly",
        long_help = "Input text containing commands for goose. Use this in lieu of the instructions argument.",
        conflicts_with = "instructions",
        conflicts_with = "recipe"
    )]
    pub input_text: Option<String>,

    /// Recipe name or full path to the recipe file
    #[arg(
        short = None,
        long = "recipe",
        value_name = "RECIPE_NAME or FULL_PATH_TO_RECIPE_FILE",
        help = "Recipe name to get recipe file or the full path of the recipe file (use --explain to see recipe details)",
        long_help = "Recipe name to get recipe file or the full path of the recipe file that defines a custom agent configuration. Use --explain to see the recipe's title, description, and parameters.",
        conflicts_with = "instructions",
        conflicts_with = "input_text"
    )]
    pub recipe: Option<String>,

    /// Additional system prompt to customize agent behavior
    #[arg(
        long = "system",
        value_name = "TEXT",
        help = "Additional system prompt to customize agent behavior",
        long_help = "Provide additional system instructions to customize the agent's behavior",
        conflicts_with = "recipe"
    )]
    pub system: Option<String>,

    #[arg(
        long,
        value_name = "KEY=VALUE",
        help = "Dynamic parameters (e.g., --params username=alice --params channel_name=goose-channel)",
        long_help = "Key-value parameters to pass to the recipe file. Can be specified multiple times.",
        action = clap::ArgAction::Append,
        value_parser = parse_key_val,
    )]
    pub params: Vec<(String, String)>,

    /// Additional sub-recipe file paths
    #[arg(
        long = "sub-recipe",
        value_name = "RECIPE",
        help = "Sub-recipe name or file path (can be specified multiple times)",
        long_help = "Specify sub-recipes to include alongside the main recipe. Can be:\n  - Recipe names from GitHub (if GOOSE_RECIPE_GITHUB_REPO is configured)\n  - Local file paths to YAML files\nCan be specified multiple times to include multiple sub-recipes.",
        action = clap::ArgAction::Append
    )]
    pub additional_sub_recipes: Vec<String>,

    /// Show the recipe title, description, and parameters
    #[arg(
        long = "explain",
        help = "Show the recipe title, description, and parameters"
    )]
    pub explain: bool,

    /// Print the rendered recipe instead of running it
    #[arg(
        long = "render-recipe",
        help = "Print the rendered recipe instead of running it."
    )]
    pub render_recipe: bool,
}

/// Output configuration options for the run command
#[derive(Args, Debug, Clone)]
pub struct OutputOptions {
    /// Quiet mode - suppress non-response output
    #[arg(
        short = 'q',
        long = "quiet",
        help = "Quiet mode. Suppress non-response output, printing only the model response to stdout"
    )]
    pub quiet: bool,

    /// Output format (text, json, stream-json)
    #[arg(
        long = "output-format",
        value_name = "FORMAT",
        help = "Output format (text, json, stream-json)",
        default_value = "text",
        value_parser = clap::builder::PossibleValuesParser::new(["text", "json", "stream-json"])
    )]
    pub output_format: String,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            quiet: false,
            output_format: "text".to_string(),
        }
    }
}

/// Model/provider override options
#[derive(Args, Debug, Clone, Default)]
pub struct ModelOptions {
    /// Provider to use for this run (overrides environment variable)
    #[arg(
        long = "provider",
        value_name = "PROVIDER",
        help = "Specify the LLM provider to use (e.g., 'openai', 'anthropic')",
        long_help = "Override the GOOSE_PROVIDER environment variable for this run. Available providers include openai, anthropic, ollama, databricks, gemini-cli, claude-code, and others."
    )]
    pub provider: Option<String>,

    /// Model to use for this run (overrides environment variable)
    #[arg(
        long = "model",
        value_name = "MODEL",
        help = "Specify the model to use (e.g., 'gpt-4o', 'claude-sonnet-4-20250514')",
        long_help = "Override the GOOSE_MODEL environment variable for this run. The model must be supported by the specified provider."
    )]
    pub model: Option<String>,
}

/// Run execution behavior options
#[derive(Args, Debug, Clone, Default)]
pub struct RunBehavior {
    /// Continue in interactive mode after processing input
    #[arg(
        short = 's',
        long = "interactive",
        help = "Continue in interactive mode after processing initial input"
    )]
    pub interactive: bool,

    /// Run without storing a session file
    #[arg(
        long = "no-session",
        help = "Run without storing a session file",
        long_help = "Execute commands without creating or using a session file. Useful for automated runs.",
        conflicts_with_all = ["resume", "name", "path"]
    )]
    pub no_session: bool,

    /// Resume a previous run
    #[arg(
        short,
        long,
        action = clap::ArgAction::SetTrue,
        help = "Resume from a previous run",
        long_help = "Continue from a previous run, maintaining the execution state and context."
    )]
    pub resume: bool,

    /// Print generation statistics after completion
    #[arg(
        long = "stats",
        help = "Print generation statistics after the run completes"
    )]
    pub stats: bool,

    /// Scheduled job ID (used internally for scheduled executions)
    #[arg(
        long = "scheduled-job-id",
        value_name = "ID",
        help = "ID of the scheduled job that triggered this execution (internal use)",
        long_help = "Internal parameter used when this run command is executed by a scheduled job. This associates the session with the schedule for tracking purposes.",
        hide = true
    )]
    pub scheduled_job_id: Option<String>,
}

async fn get_or_create_session_id(
    identifier: Option<Identifier>,
    resume: bool,
    no_session: bool,
    goose_mode: GooseMode,
) -> Result<Option<String>> {
    if no_session {
        return Ok(None);
    }

    let session_manager = SessionManager::instance();

    let resolved_id = if resume {
        let Some(id) = identifier else {
            let sessions = session_manager
                .list_sessions_by_types(&[SessionType::User])
                .await?;
            let session_id = sessions
                .first()
                .map(|s| s.id.clone())
                .ok_or_else(|| anyhow::anyhow!("No session found to resume"))?;
            return Ok(Some(session_id));
        };

        if let Some(session_id) = id.session_id {
            session_id
        } else if let Some(name) = id.name {
            let sessions = session_manager.list_sessions().await?;
            sessions
                .into_iter()
                .find(|s| s.name == name || s.id == name)
                .map(|s| s.id)
                .ok_or_else(|| anyhow::anyhow!("No session found with name '{}'", name))?
        } else if let Some(path) = id.path {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    anyhow::anyhow!("Could not extract session ID from path: {:?}", path)
                })?
        } else {
            return Err(anyhow::anyhow!("Invalid identifier"));
        }
    } else {
        let Some(id) = identifier else {
            let session = session_manager
                .create_session(
                    std::env::current_dir()?,
                    "CLI Session".to_string(),
                    SessionType::User,
                    goose_mode,
                )
                .await?;
            return Ok(Some(session.id));
        };

        if id.session_id.is_some() {
            return Err(anyhow::anyhow!("Cannot use --session-id without --resume"));
        }

        let has_user_provided_name = id.name.is_some();
        let name = id.name.unwrap_or_else(|| "CLI Session".to_string());
        let session = session_manager
            .create_session(
                std::env::current_dir()?,
                name.clone(),
                SessionType::User,
                goose_mode,
            )
            .await?;

        if has_user_provided_name {
            session_manager
                .update(&session.id)
                .user_provided_name(name)
                .apply()
                .await?;
        }

        return Ok(Some(session.id));
    };

    Ok(Some(resolved_id))
}

async fn lookup_session_id(identifier: Identifier) -> Result<String> {
    let session_manager = SessionManager::instance();

    if let Some(session_id) = identifier.session_id {
        Ok(session_id)
    } else if let Some(name) = identifier.name {
        let sessions = session_manager.list_sessions().await?;
        sessions
            .into_iter()
            .find(|s| s.name == name || s.id == name)
            .map(|s| s.id)
            .ok_or_else(|| anyhow::anyhow!("No session found with name '{}'", name))
    } else if let Some(path) = identifier.path {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Could not extract session ID from path: {:?}", path))
    } else {
        Err(anyhow::anyhow!("No identifier provided"))
    }
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => Ok((key.to_string(), value.to_string())),
        None => Err(format!("invalid KEY=VALUE: {}", s)),
    }
}

#[derive(Subcommand)]
enum SessionCommand {
    #[command(about = "List all available sessions")]
    List {
        #[arg(
            short,
            long,
            help = "Output format (text, json)",
            default_value = "text"
        )]
        format: String,

        #[arg(
            long = "ascending",
            help = "Sort by date in ascending order (oldest first)",
            long_help = "Sort sessions by date in ascending order (oldest first). Default is descending order (newest first)."
        )]
        ascending: bool,

        #[arg(
            short = 'w',
            short_alias = 'p',
            long = "working_dir",
            help = "Filter sessions by working directory"
        )]
        working_dir: Option<PathBuf>,

        #[arg(short = 'l', long = "limit", help = "Limit the number of results")]
        limit: Option<usize>,
    },
    #[command(about = "Remove sessions. Runs interactively if no ID, name, or regex is provided.")]
    Remove {
        #[command(flatten)]
        identifier: Option<Identifier>,
        #[arg(
            short = 'r',
            long,
            help = "Regex for removing matched sessions (optional)"
        )]
        regex: Option<String>,
    },
    #[command(about = "Export a session")]
    Export {
        #[command(flatten)]
        identifier: Option<Identifier>,

        #[arg(
            short,
            long,
            help = "Output file path (default: stdout)",
            long_help = "Path to save the exported Markdown. If not provided, output will be sent to stdout"
        )]
        output: Option<PathBuf>,

        #[arg(
            long = "format",
            value_name = "FORMAT",
            help = "Output format (markdown, json, yaml)",
            default_value = "markdown"
        )]
        format: String,

        #[arg(
            long = "nostr",
            help = "Publish the JSON session export as an encrypted Nostr event and print a Goose share link"
        )]
        nostr: bool,

        #[arg(
            long = "relay",
            value_name = "RELAY",
            help = "Nostr relay URL to publish to (can be specified multiple times)",
            action = clap::ArgAction::Append
        )]
        relays: Vec<String>,
    },
    #[command(
        about = "Import a session from JSON, a Claude Code / Codex / Pi .jsonl, or an encrypted Nostr share link"
    )]
    Import {
        #[arg(
            help = "Path to a goose session export, a Claude Code, Codex, or Pi .jsonl transcript, or a goose://sessions/nostr share link"
        )]
        input: String,

        #[arg(long = "nostr", help = "Treat input as an encrypted Nostr share link")]
        nostr: bool,
    },
    #[command(name = "diagnostics")]
    Diagnostics {
        #[command(flatten)]
        identifier: Option<Identifier>,

        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum SchedulerCommand {
    #[command(about = "Add a new scheduled job")]
    Add {
        #[arg(
            long = "schedule-id",
            alias = "id",
            help = "Unique ID for the recurring scheduled job"
        )]
        schedule_id: String,
        #[arg(
            long,
            help = "Cron expression for the schedule",
            long_help = "Cron expression for when to run the job. Examples:\n  '0 * * * *'     - Every hour at minute 0\n  '0 */2 * * *'   - Every 2 hours\n  '@hourly'       - Every hour (shorthand)\n  '0 9 * * *'     - Every day at 9:00 AM\n  '0 9 * * 1'     - Every Monday at 9:00 AM\n  '0 0 1 * *'     - First day of every month at midnight"
        )]
        cron: String,
        #[arg(
            long,
            help = "Recipe source (path to file, or base64 encoded recipe string)"
        )]
        recipe_source: String,
        #[arg(
            long,
            value_name = "KEY=VALUE",
            help = "Recipe parameter in KEY=VALUE format (can be specified multiple times)",
            action = clap::ArgAction::Append,
            value_parser = parse_key_val,
        )]
        params: Vec<(String, String)>,
    },
    #[command(about = "List all scheduled jobs")]
    List {},
    #[command(about = "Remove a scheduled job by ID")]
    Remove {
        #[arg(
            long = "schedule-id",
            alias = "id",
            help = "ID of the scheduled job to remove (removes the recurring schedule)"
        )]
        schedule_id: String,
    },
    /// List sessions created by a specific schedule
    #[command(about = "List sessions created by a specific schedule")]
    Sessions {
        /// ID of the schedule
        #[arg(long = "schedule-id", alias = "id", help = "ID of the schedule")]
        schedule_id: String,
        #[arg(short = 'l', long, help = "Maximum number of sessions to return")]
        limit: Option<usize>,
    },
    #[command(about = "Run a scheduled job immediately")]
    RunNow {
        /// ID of the schedule to run
        #[arg(long = "schedule-id", alias = "id", help = "ID of the schedule to run")]
        schedule_id: String,
    },
    /// Check status of scheduler services (deprecated - no external services needed)
    #[command(about = "[Deprecated] Check status of scheduler services")]
    ServicesStatus {},
    /// Stop scheduler services (deprecated - no external services needed)
    #[command(about = "[Deprecated] Stop scheduler services")]
    ServicesStop {},
    /// Show cron expression examples and help
    #[command(about = "Show cron expression examples and help")]
    CronHelp {},
}

#[derive(Subcommand)]
enum GatewayCommand {
    #[command(about = "Show gateway status")]
    Status {},

    #[command(about = "Start a gateway")]
    Start {
        #[arg(help = "Gateway type (e.g., 'telegram')")]
        gateway_type: String,

        #[arg(
            long = "bot-token",
            help = "Bot token for the gateway platform",
            long_help = "Authentication token for the gateway platform (e.g., Telegram bot token)"
        )]
        bot_token: String,
    },

    #[command(about = "Stop a running gateway")]
    Stop {
        #[arg(help = "Gateway type to stop (e.g., 'telegram')")]
        gateway_type: String,
    },

    #[command(about = "Generate a pairing code for a gateway")]
    Pair {
        #[arg(help = "Gateway type to generate pairing code for")]
        gateway_type: String,
    },
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Install a plugin from a git repository URL
    #[command(about = "Install a plugin from a git repository URL")]
    Install {
        #[arg(
            long,
            help = "Automatically update this plugin before plugin skills are loaded"
        )]
        auto_update: bool,

        #[arg(help = "URL to a git repository containing a supported plugin")]
        url: String,
    },

    /// Update an installed git-backed plugin
    #[command(about = "Update an installed git-backed plugin")]
    Update {
        #[arg(help = "Name of the installed plugin to update")]
        name: String,
    },
}

#[derive(Subcommand)]
enum SkillsCommand {
    /// List all skills available to the goose agent
    #[command(about = "List all skills available to the goose agent")]
    List,
}

#[derive(Subcommand)]
enum RecipeCommand {
    /// Validate a recipe file
    #[command(about = "Validate a recipe")]
    Validate {
        /// Recipe name to get recipe file to validate
        #[arg(help = "recipe name to get recipe file or full path to the recipe file to validate")]
        recipe_name: String,
    },

    /// Generate a deeplink for a recipe file
    #[command(about = "Generate a deeplink for a recipe")]
    Deeplink {
        /// Recipe name to get recipe file to generate deeplink
        #[arg(
            help = "recipe name to get recipe file or full path to the recipe file to generate deeplink"
        )]
        recipe_name: String,
        /// Recipe parameters in key=value format (can be specified multiple times)
        #[arg(
            short = 'p',
            long = "param",
            value_name = "KEY=VALUE",
            help = "Recipe parameter in key=value format (can be specified multiple times)"
        )]
        params: Vec<String>,
    },

    /// Open a recipe in Goose Desktop
    #[command(about = "Open a recipe in Goose Desktop")]
    Open {
        /// Recipe name to get recipe file to open
        #[arg(help = "recipe name or full path to the recipe file")]
        recipe_name: String,
        /// Recipe parameters in key=value format (can be specified multiple times)
        #[arg(
            short = 'p',
            long = "param",
            value_name = "KEY=VALUE",
            help = "Recipe parameter in key=value format (can be specified multiple times)"
        )]
        params: Vec<String>,
    },

    /// List available recipes
    #[command(about = "List available recipes")]
    List {
        /// Output format (text, json)
        #[arg(
            long = "format",
            value_name = "FORMAT",
            help = "Output format (text, json)",
            default_value = "text"
        )]
        format: String,

        /// Show verbose information including recipe descriptions
        #[arg(
            short,
            long,
            help = "Show verbose information including recipe descriptions"
        )]
        verbose: bool,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Configure goose settings
    #[command(about = "Configure goose settings")]
    Configure {},

    /// Display goose configuration information
    #[command(about = "Display goose information")]
    Info {
        /// Show verbose information including current configuration
        #[arg(short, long, help = "Show verbose information including config.yaml")]
        verbose: bool,
        #[arg(long, help = "Test provider connection and show status")]
        check: bool,
    },

    #[command(about = "Check that your Goose setup is working")]
    Doctor {},

    /// Manage system prompts and behaviors
    #[command(about = "Run one of the mcp servers bundled with goose")]
    Mcp {
        #[arg(value_parser = clap::value_parser!(McpCommand))]
        server: McpCommand,
    },

    /// Run goose as an ACP (Agent Client Protocol) agent
    #[command(about = "Run goose as an ACP agent server on stdio")]
    Acp {
        /// Add builtin extensions by name
        #[arg(
            long = "with-builtin",
            value_name = "NAME",
            help = "Add builtin extensions by name (e.g., 'developer' or multiple: 'developer,github')",
            long_help = "Add one or more builtin extensions that are bundled with goose by specifying their names, comma-separated",
            value_delimiter = ','
        )]
        builtins: Vec<String>,

        #[arg(long, help = "Enable scheduled recipe execution")]
        enable_scheduler: bool,
    },

    /// Share or connect to agents peer-to-peer over iroh
    #[cfg(feature = "roaming")]
    #[command(about = "Share or connect to agents peer-to-peer (roaming)")]
    Roam {
        #[command(subcommand)]
        command: RoamCommand,
    },

    /// Start ACP server over HTTP and WebSocket
    #[command(about = "Start ACP server over HTTP and WebSocket")]
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        #[arg(long, default_value = "3284")]
        port: u16,

        #[arg(long, help = "Serve ACP over TLS")]
        tls: bool,

        #[arg(long = "tls-cert-path", value_name = "PATH")]
        tls_cert_path: Option<String>,

        #[arg(long = "tls-key-path", value_name = "PATH")]
        tls_key_path: Option<String>,

        #[arg(long, value_enum, default_value_t = ServePlatform::Cli)]
        platform: ServePlatform,

        #[arg(
            long = "with-builtin",
            value_name = "NAME",
            help = "Add builtin extensions by name (e.g., 'developer' or multiple: 'developer,github')",
            long_help = "Add one or more builtin extensions that are bundled with goose by specifying their names, comma-separated",
            value_delimiter = ',',
            action = clap::ArgAction::Append
        )]
        builtins: Vec<String>,

        #[arg(
            long = "dangerously-unauthenticated",
            help = "Start the ACP endpoint without requiring GOOSE_SERVER__SECRET_KEY"
        )]
        dangerously_unauthenticated: bool,

        #[arg(
            long = "allowed-origin",
            value_name = "ORIGIN",
            action = clap::ArgAction::Append,
            help = "Allow an exact Origin value for ACP CORS; may be specified multiple times and replaces the default loopback origins"
        )]
        allowed_origins: Vec<String>,

        #[arg(long, help = "Enable scheduled recipe execution")]
        enable_scheduler: bool,

        /// Also expose this server over goose roam (p2p) so paired devices can connect remotely
        #[cfg(feature = "roaming")]
        #[arg(
            long,
            help = "Also expose this server over goose roam (p2p) so paired devices can connect remotely"
        )]
        roam: bool,
    },

    /// Start or resume interactive chat sessions
    #[command(
        about = "Start or resume interactive chat sessions",
        visible_alias = "s"
    )]
    Session {
        #[command(subcommand)]
        command: Option<SessionCommand>,

        #[command(flatten)]
        identifier: Option<Identifier>,

        /// Resume a previous session
        #[arg(
            short,
            long,
            help = "Resume a previous session (last used or specified by --name/--session-id)",
            long_help = "Continue from a previous session. If --name or --session-id is provided, resumes that specific session. Otherwise, resumes the most recently used session."
        )]
        resume: bool,

        /// Fork a previous session (creates new session with copied history)
        #[arg(
            long,
            requires = "resume",
            help = "Fork a previous session (creates new session with copied history)",
            long_help = "Create a new session by copying all messages from a previous session. Must be used with --resume. If --name or --session-id is provided, forks that specific session. Otherwise, forks the most recently used session."
        )]
        fork: bool,

        /// Open the session's conversation in $EDITOR before starting
        #[arg(
            long,
            requires = "resume",
            help = "Edit the session conversation in $EDITOR before starting",
            long_help = "Open the session's conversation in your editor ($VISUAL / $EDITOR / vi) for modification before resuming. When combined with --fork, creates a new session from the edited result."
        )]
        edit: bool,

        /// Show message history when resuming
        #[arg(
            long,
            help = "Show previous messages when resuming a session",
            requires = "resume"
        )]
        history: bool,

        /// Additional system prompt to customize agent behavior
        #[arg(
            long = "system",
            value_name = "TEXT",
            help = "Additional system prompt to customize agent behavior",
            long_help = "Provide additional system instructions to customize the agent's behavior"
        )]
        system: Option<String>,

        #[command(flatten)]
        session_opts: SessionOptions,

        #[command(flatten)]
        extension_opts: ExtensionOptions,

        #[command(flatten)]
        model_opts: ModelOptions,
    },

    /// Execute commands from an instruction file
    #[command(about = "Execute commands from an instruction file or stdin")]
    Run {
        #[command(flatten)]
        input_opts: InputOptions,

        #[command(flatten)]
        identifier: Option<Identifier>,

        #[command(flatten)]
        run_behavior: RunBehavior,

        #[command(flatten)]
        session_opts: SessionOptions,

        #[command(flatten)]
        extension_opts: ExtensionOptions,

        #[command(flatten)]
        output_opts: OutputOptions,

        #[command(flatten)]
        model_opts: ModelOptions,
    },

    /// Recipe utilities for validation and deeplinking
    #[command(about = "Recipe utilities for validation and deeplinking")]
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },

    /// Skill utilities
    #[command(about = "Skill utilities")]
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },

    /// Manage plugins
    #[command(about = "Manage plugins")]
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },

    /// Manage scheduled jobs
    #[command(about = "Manage scheduled jobs", visible_alias = "sched")]
    Schedule {
        #[command(subcommand)]
        command: SchedulerCommand,
    },

    /// Manage gateways for external platform integrations (e.g., Telegram)
    #[command(
        about = "Manage gateways for external platform integrations",
        visible_alias = "gw"
    )]
    Gateway {
        #[command(subcommand)]
        command: GatewayCommand,
    },

    /// Update the goose CLI version
    #[cfg(feature = "update")]
    #[command(about = "Update the goose CLI version")]
    Update {
        /// Update to canary version
        #[arg(
            short,
            long,
            help = "Update to canary version",
            long_help = "Update to the latest canary version of the goose CLI, otherwise updates to the latest stable version."
        )]
        canary: bool,

        /// Enforce to re-configure goose during update
        #[arg(short, long, help = "Enforce to re-configure goose during update")]
        reconfigure: bool,
    },

    /// Terminal-integrated session (one session per terminal)
    #[command(
        about = "Terminal-integrated goose session",
        long_about = "Runs a goose session tied to your terminal window.\n\
                      Each terminal maintains its own persistent session that resumes automatically.\n\n\
                      Setup:\n  \
                        eval \"$(goose term init zsh)\"  # zsh/bash\n  \
                        let init = ($nu.cache-dir | path join \"goose-term-init.nu\"); ^goose term init nu | save --force $init; source $init\n\n\
                      Usage:\n  \
                        goose term run \"list files in this directory\"\n  \
                        @goose \"create a python script\"  # using alias\n  \
                        @g \"quick question\"  # short alias"
    )]
    Term {
        #[command(subcommand)]
        command: TermCommand,
    },

    /// Manage local inference models
    #[cfg(feature = "local-inference")]
    #[command(about = "Manage local inference models", visible_alias = "lm")]
    LocalModels {
        #[command(subcommand)]
        command: LocalModelsCommand,
    },

    /// Generate completions for various shells
    #[command(
        about = "Generate the autocompletion script or Nushell module for the specified shell"
    )]
    Completion {
        #[arg(value_enum)]
        shell: CompletionShell,

        #[arg(long, default_value = "goose", help = "Provide a custom binary name")]
        bin_name: String,
    },

    /// Local code review.
    ///
    /// Discovers `**/.agents/checks/*.md` subagent reviewers and
    /// `**/.agents/REVIEW.md` scoped prompt overrides, builds a review
    /// request from the working tree (or an explicit diff range), and
    /// runs the review through goose.
    #[command(about = "Review the current diff using goose")]
    Review {
        /// Diff range to review (e.g. "main...HEAD"). Defaults to the working
        /// tree vs HEAD.
        #[arg(value_name = "RANGE")]
        range: Option<String>,

        /// Path to a Markdown file with a custom base review prompt. Replaces
        /// the embedded default prompt.
        #[arg(long = "prompt", value_name = "FILE")]
        prompt: Option<PathBuf>,

        /// Default model used for the main review agent and for any check
        /// that does not declare its own `model:` in frontmatter.
        #[arg(long = "model", value_name = "MODEL")]
        model: Option<String>,

        /// Provider for the main review agent.
        #[arg(long = "provider", value_name = "PROVIDER")]
        provider: Option<String>,

        /// Force every discovered check to use this model, regardless of
        /// the check's own `model:` field.
        #[arg(long = "override-model", value_name = "MODEL")]
        override_model: Option<String>,

        /// Default `turn-limit` for orchestrated main-pass subprocesses and
        /// for checks that do not declare their own. Does not cap the legacy
        /// `--no-orchestrate` in-process main agent.
        #[arg(long = "turn-limit", value_name = "N")]
        turn_limit: Option<usize>,

        /// Print the assembled review prompt and discovered checks instead of
        /// running the review.
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Suppress non-result output from the underlying agent.
        #[arg(long, short = 'q')]
        quiet: bool,

        /// Disable the Rust-driven parallel orchestrator and fall back to
        /// the single-prompt path that asks the main agent to delegate
        /// each check via `delegate(... async: true ...)`. The default
        /// orchestrator dispatches one `goose run` subprocess per check
        /// (capped at 4 concurrent), bounding wall-clock to the slowest
        /// single check rather than waiting on the model to issue
        /// dispatches.
        /// Checks with an explicit tool allowlist require the default orchestrator.
        #[arg(long = "no-orchestrate")]
        no_orchestrate: bool,

        /// Additional free-form instructions to prepend to the review
        /// (e.g. PR intent, commit-message context, "this is a refactor,
        /// flag any behavior change"). Mirrors `amp review --instructions`
        /// for drop-in compatibility with existing reviewer wrappers.
        #[arg(long = "instructions", short = 'i', value_name = "TEXT")]
        instructions: Option<String>,

        /// Restrict the review to a specific set of files. Other files in
        /// the diff are still passed to the agent for context but are
        /// excluded from the assembled diff sent to checks. Mirrors
        /// `amp review --files`.
        #[arg(long = "files", short = 'f', value_name = "FILE", num_args = 1..)]
        files: Vec<String>,

        /// Only run checks whose `name` matches one of these. Other
        /// discovered checks are skipped. Mirrors `amp review --check-filter`.
        #[arg(long = "check-filter", short = 'c', value_name = "NAME", num_args = 1..)]
        check_filter: Vec<String>,

        /// Alternate directory to search for `.agents/checks/*.md` instead
        /// of the repo root. Mirrors `amp review --check-scope`.
        #[arg(long = "check-scope", short = 's', value_name = "DIR")]
        check_scope: Option<PathBuf>,

        /// Skip the main correctness pass and only run check subagents.
        /// Mirrors `amp review --checks-only`.
        #[arg(long = "checks-only")]
        checks_only: bool,

        /// Print only the diff summary; skip the full review.
        /// Mirrors `amp review --summary-only`.
        #[arg(long = "summary-only")]
        summary_only: bool,

        /// Minimum severity to display. Findings below this rank are
        /// dropped from the output. Default is `medium`, matching
        /// Amp's CLI which hides `low` from review output. Pass
        /// `--severity low` to surface every finding.
        #[arg(long = "severity", value_name = "LEVEL", default_value = "medium")]
        severity: String,
    },
    #[command(
        name = "validate-extensions",
        about = "Validate a bundled-extensions.json file",
        hide = true
    )]
    ValidateExtensions {
        #[arg(help = "Path to the bundled-extensions.json file")]
        file: PathBuf,
    },

    #[command(
        name = "mcp-probe",
        about = "Start a Goose MCP session without an LLM and inspect a stdio MCP server",
        hide = true
    )]
    McpProbe {
        #[arg(help = "Stdio MCP server command to inspect")]
        extension: String,

        #[arg(
            long,
            value_name = "PATH|-",
            help = "JSON probe script; use - for stdin"
        )]
        script: Option<String>,
    },
}

#[cfg(feature = "local-inference")]
#[derive(Subcommand)]
enum LocalModelsCommand {
    /// Search HuggingFace for local models
    #[command(about = "Search HuggingFace for local GGUF and MLX models")]
    Search {
        /// Search query
        query: Option<String>,

        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Only include repos whose id starts with this prefix
        #[arg(long)]
        repo_prefix: Option<String>,

        /// Only include repos whose id ends with this suffix
        #[arg(long)]
        repo_suffix: Option<String>,

        /// Only include variants whose quantization contains this text
        #[arg(long)]
        quant: Option<String>,

        /// Override available memory used for recommendations, in GB
        #[arg(long)]
        ram_gb: Option<f64>,

        /// Print results as JSON
        #[arg(long)]
        json: bool,
    },

    /// Download a model from HuggingFace
    #[command(about = "Download a local model from a search result")]
    Download {
        /// Model spec/download id, e.g. user/repo:Q4_K_M or user/repo
        spec: String,
    },

    /// List downloaded local models
    #[command(about = "List downloaded local models")]
    List,

    /// Delete a downloaded model
    #[command(about = "Delete a downloaded local model")]
    Delete {
        /// Model ID to delete
        id: String,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Print shell initialization script
    #[command(
        about = "Print shell initialization script",
        long_about = "Prints shell configuration to set up terminal-integrated sessions.\n\
                      Each terminal gets a persistent goose session that automatically resumes.\n\n\
                      Setup:\n  \
                        echo 'eval \"$(goose term init zsh)\"' >> ~/.zshrc\n  \
                        source ~/.zshrc\n\n\
                        Nushell:\n  \
                        let init = ($nu.cache-dir | path join \"goose-term-init.nu\")\n  \
                        ^goose term init nu | save --force $init\n  \
                        source $init\n\n\
                      With --default (anything typed that isn't a command goes to goose):\n  \
                        echo 'eval \"$(goose term init zsh --default)\"' >> ~/.zshrc\n  \
                        ^goose term init nu --default | save --force $init"
    )]
    Init {
        /// Shell type (bash, zsh, fish, nu, powershell)
        #[arg(value_enum)]
        shell: Shell,

        #[arg(short, long, help = "Name for the terminal session")]
        name: Option<String>,

        /// Make goose the default handler for unknown commands
        #[arg(
            long = "default",
            help = "Make goose the default handler for unknown commands",
            long_help = "When enabled, anything you type that isn't a valid command will be sent to goose. Supported for zsh, bash, and nu."
        )]
        default: bool,
    },

    /// Log a shell command (called by shell hook)
    #[command(about = "Log a shell command to the session", hide = true)]
    Log {
        /// The command that was executed
        command: String,
    },

    /// Run a prompt in the terminal session
    #[command(
        about = "Run a prompt in the terminal session",
        long_about = "Run a prompt in the terminal-integrated session.\n\n\
                      Examples:\n  \
                        goose term run list files in this directory\n  \
                        @goose list files  # using alias\n  \
                        @g why did that fail  # short alias"
    )]
    Run {
        /// The prompt to send to goose (multiple words allowed without quotes)
        #[arg(required = true, num_args = 1..)]
        prompt: Vec<String>,
    },

    /// Print session info for prompt integration
    #[command(
        about = "Print session info for prompt integration",
        long_about = "Prints compact session info (token usage, model) for shell prompt integration.\n\
                      Example output: ●○○○○ sonnet"
    )]
    Info,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum CliProviderVariant {
    OpenAi,
    Databricks,
    Ollama,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    #[value(alias = "pwsh")]
    Powershell,
    #[value(alias = "nushell")]
    Nu,
    Zsh,
}

impl CompletionShell {
    fn generate(self, cmd: &mut clap::Command, bin_name: &str, writer: &mut dyn std::io::Write) {
        match self {
            CompletionShell::Bash => generate(ClapShell::Bash, cmd, bin_name, writer),
            CompletionShell::Elvish => generate(ClapShell::Elvish, cmd, bin_name, writer),
            CompletionShell::Fish => generate(ClapShell::Fish, cmd, bin_name, writer),
            CompletionShell::Powershell => generate(ClapShell::PowerShell, cmd, bin_name, writer),
            CompletionShell::Nu => generate(ClapNushell, cmd, bin_name, writer),
            CompletionShell::Zsh => generate(ClapShell::Zsh, cmd, bin_name, writer),
        }
    }
}

#[derive(Debug)]
pub struct InputConfig {
    pub contents: Option<String>,
    pub additional_system_prompt: Option<String>,
}

fn get_command_name(command: &Option<Command>) -> &'static str {
    match command {
        Some(Command::Configure {}) => "configure",
        Some(Command::Doctor {}) => "doctor",
        Some(Command::Info { .. }) => "info",
        Some(Command::Mcp { .. }) => "mcp",
        Some(Command::Acp { .. }) => "acp",
        #[cfg(feature = "roaming")]
        Some(Command::Roam { .. }) => "roam",
        Some(Command::Serve { .. }) => "serve",
        Some(Command::Session { .. }) => "session",
        Some(Command::Run { .. }) => "run",
        Some(Command::Gateway { .. }) => "gateway",
        Some(Command::Schedule { .. }) => "schedule",
        #[cfg(feature = "update")]
        Some(Command::Update { .. }) => "update",
        Some(Command::Recipe { .. }) => "recipe",
        Some(Command::Skills { .. }) => "skills",
        Some(Command::Plugin { .. }) => "plugin",
        Some(Command::Term { .. }) => "term",
        #[cfg(feature = "local-inference")]
        Some(Command::LocalModels { .. }) => "local-models",
        Some(Command::Completion { .. }) => "completion",
        Some(Command::Review { .. }) => "review",
        Some(Command::ValidateExtensions { .. }) => "validate-extensions",
        Some(Command::McpProbe { .. }) => "mcp-probe",
        None => "default_session",
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpProbeScript {
    #[serde(default)]
    steps: Vec<McpProbeStep>,
    elicitation: Option<McpProbeElicitation>,
    #[serde(default)]
    oauth: goose::oauth::OAuthFlowConfig,
    protocol_version: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
enum McpProbeStep {
    ListTools,
    ListPrompts,
    ListResources,
    CallTool {
        name: String,
        #[serde(default)]
        arguments: serde_json::Map<String, serde_json::Value>,
    },
}

#[derive(Clone, serde::Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
enum McpProbeElicitation {
    Accept { content: serde_json::Value },
    AcceptSchemaDefaults,
    Decline,
    Cancel,
}

async fn handle_mcp_probe(extension_command: String, script_path: Option<String>) -> Result<()> {
    use goose::agents::{Agent, AgentConfig, ToolCallContext};
    use goose::config::ExtensionConfig;
    use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
    use tokio_util::sync::CancellationToken;

    let script = if let Some(path) = script_path {
        let json = if path == "-" {
            let mut json = String::new();
            std::io::stdin().read_to_string(&mut json)?;
            json
        } else {
            std::fs::read_to_string(path)?
        };
        serde_json::from_str::<McpProbeScript>(&json)?
    } else {
        McpProbeScript {
            steps: vec![
                McpProbeStep::ListTools,
                McpProbeStep::ListPrompts,
                McpProbeStep::ListResources,
            ],
            elicitation: None,
            oauth: goose::oauth::OAuthFlowConfig::default(),
            protocol_version: None,
        }
    };

    let mut extension = if url::Url::parse(&extension_command)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    {
        crate::session::CliSession::parse_streamable_http_extension(
            &extension_command,
            goose::config::DEFAULT_EXTENSION_TIMEOUT,
        )
    } else {
        crate::session::CliSession::parse_stdio_extension(&extension_command)?
    };
    match &mut extension {
        ExtensionConfig::Stdio { name, .. } | ExtensionConfig::StreamableHttp { name, .. } => {
            *name = "probe".to_string();
        }
        _ => unreachable!("MCP probe only creates stdio or streamable HTTP extensions"),
    }

    if let Some(client_id) = &script.oauth.client_id {
        std::env::set_var("GOOSE_MCP_OAUTH_CLIENT_ID", client_id);
    }
    if let Some(client_secret) = &script.oauth.client_secret {
        std::env::set_var("GOOSE_MCP_OAUTH_CLIENT_SECRET", client_secret);
    }
    if let Some(client_metadata_url) = &script.oauth.client_metadata_url {
        std::env::set_var("GOOSE_MCP_OAUTH_CLIENT_METADATA_URL", client_metadata_url);
    }

    let config = goose::config::Config::global();
    let mut agent_config = AgentConfig::new(
        std::sync::Arc::new(SessionManager::instance()),
        goose::config::permission::PermissionManager::instance(),
        None,
        config.get_goose_mode().unwrap_or_default(),
        true,
        GoosePlatform::GooseCli,
    );
    if let Some(protocol_version) = script.protocol_version.as_deref() {
        agent_config.mcp_protocol_version = Some(serde_json::from_value(
            serde_json::Value::String(protocol_version.to_string()),
        )?);
    }
    if let Some(action) = script.elicitation.clone() {
        agent_config.elicitation_handler =
            Some(std::sync::Arc::new(move |request| match &action {
                McpProbeElicitation::Accept { content } => {
                    ElicitResult::new(ElicitationAction::Accept).with_content(content.clone())
                }
                McpProbeElicitation::AcceptSchemaDefaults => {
                    let content = match request {
                        ElicitRequestParams::FormElicitationParams {
                            requested_schema, ..
                        } => serde_json::to_value(requested_schema)
                            .ok()
                            .and_then(|schema| schema.get("properties").cloned())
                            .and_then(|properties| properties.as_object().cloned())
                            .map(|properties| {
                                properties
                                    .into_iter()
                                    .filter_map(|(name, schema)| {
                                        schema.get("default").cloned().map(|value| (name, value))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        _ => serde_json::Map::new(),
                    };
                    ElicitResult::new(ElicitationAction::Accept)
                        .with_content(serde_json::Value::Object(content))
                }
                McpProbeElicitation::Decline => ElicitResult::new(ElicitationAction::Decline),
                McpProbeElicitation::Cancel => ElicitResult::new(ElicitationAction::Cancel),
            }));
    }
    let agent = Agent::with_config(agent_config);
    let session = agent
        .config
        .session_manager
        .create_session(
            std::env::current_dir()?,
            "MCP Probe".to_string(),
            goose::session::session_manager::SessionType::Hidden,
            agent.config.goose_mode,
        )
        .await?;
    let session_id = session.id.as_str();
    agent.add_extension(extension, session_id).await?;

    let mut results = Vec::new();
    for step in script.steps {
        let result = match step {
            McpProbeStep::ListTools => serde_json::json!({
                "action": "listTools",
                "result": agent.extension_manager.list_tools_from_extension(
                    session_id,
                    "probe",
                    CancellationToken::new(),
                ).await?,
            }),
            McpProbeStep::ListPrompts => serde_json::json!({
                "action": "listPrompts",
                "result": agent.extension_manager.list_prompts_from_extension(
                    session_id,
                    "probe",
                    CancellationToken::new(),
                ).await?,
            }),
            McpProbeStep::ListResources => serde_json::json!({
                "action": "listResources",
                "result": agent.extension_manager.list_resources_result_from_extension(
                    session_id,
                    "probe",
                    CancellationToken::new(),
                ).await?,
            }),
            McpProbeStep::CallTool { name, arguments } => {
                let scoped_name = format!("probe__{name}");
                let ctx = ToolCallContext::new(
                    session_id.to_string(),
                    Some(std::env::current_dir()?),
                    Some("mcp-probe-tool-call".to_string()),
                );
                let result = agent
                    .extension_manager
                    .dispatch_tool_call(
                        &ctx,
                        rmcp::model::CallToolRequestParams::new(scoped_name)
                            .with_arguments(arguments),
                        CancellationToken::new(),
                    )
                    .await?
                    .result
                    .await?;
                serde_json::json!({ "action": "callTool", "name": name, "result": result })
            }
        };
        results.push(result);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "results": results }))?
    );
    Ok(())
}

async fn handle_mcp_command(server: McpCommand) -> Result<()> {
    let name = server.name();
    let _ = crate::logging::setup_logging(Some(&format!("mcp-{name}")));
    match server {
        McpCommand::AutoVisualiser => serve(AutoVisualiserRouter::new()).await?,
        McpCommand::ComputerController => serve(ComputerControllerServer::new()).await?,
        McpCommand::Memory => serve(MemoryServer::new()).await?,
        McpCommand::Tutorial => serve(TutorialServer::new()).await?,
    }
    Ok(())
}

struct ServeCommandArgs {
    host: String,

    port: u16,
    tls: bool,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
    platform: ServePlatform,
    builtins: Vec<String>,
    dangerously_unauthenticated: bool,
    allowed_origins: Vec<String>,
    enable_scheduler: bool,
    #[cfg(feature = "roaming")]
    roam: bool,
}

#[cfg(feature = "roaming")]
type RoamShareSlot =
    std::sync::Arc<tokio::sync::RwLock<Option<std::sync::Arc<goose_roaming::RoamingNode>>>>;

#[cfg(feature = "roaming")]
fn spawn_roam_share(
    server: std::sync::Arc<goose::acp::server_factory::AcpServer>,
) -> RoamShareSlot {
    use crate::commands::roam::try_acquire_roam_lock_owner;

    let slot = RoamShareSlot::default();
    let task_slot = slot.clone();
    tokio::spawn(async move {
        let mut standing_by = false;
        loop {
            match try_acquire_roam_lock_owner() {
                Ok(Some(lock)) => match start_roam_share(server.clone()).await {
                    Ok(node) => {
                        *task_slot.write().await = Some(node);
                        let _lock = lock;
                        std::future::pending::<()>().await;
                    }
                    Err(error) => {
                        tracing::error!("roam share failed to start: {error}");
                        drop(lock);
                    }
                },
                Ok(None) => {
                    if !standing_by {
                        standing_by = true;
                        eprintln!(
                            "another goose process owns the roaming endpoint; standing by to take over if it exits"
                        );
                    }
                }
                // A real failure (unwritable data dir, filesystem error) will
                // not fix itself: surface it and stop instead of retrying as
                // if the endpoint were merely busy.
                Err(error) => {
                    eprintln!("roaming disabled: cannot acquire the endpoint lock: {error:#}");
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });
    slot
}

#[cfg(feature = "roaming")]
async fn start_roam_share(
    server: std::sync::Arc<goose::acp::server_factory::AcpServer>,
) -> Result<std::sync::Arc<goose_roaming::RoamingNode>> {
    use crate::commands::roam::{
        directory_path, load_identity, resolve_relay_settings, trust_path,
    };
    use crate::commands::roam_full_bridge::FullAcpBridge;
    use goose::config::paths::Paths;
    use goose_roaming::{RoamingConfig, RoamingNode, TrustBook};
    use std::sync::Arc;

    let status_path = Paths::data_dir().join("roam/serve.json");
    let _ = std::fs::remove_file(&status_path);

    let identity = load_identity()?;
    let node = RoamingNode::bind(RoamingConfig {
        identity,
        relay: resolve_relay_settings()?,
        trust: TrustBook::new(),
        trust_path: Some(trust_path()),
        directory: goose_roaming::Directory::persistent_owned(directory_path()),
        bind_addr: None,
        relay_tls: None,
    })
    .await?;

    let agent_id = node.endpoint_id().to_string();
    // Roaming sessions run where `goose serve` was started: the connector's
    // machine-local path is meaningless on this host, and the serve-wide
    // server keeps `session_cwd: None` for local ACP clients.
    let session_cwd =
        std::env::current_dir().map_err(|e| anyhow::anyhow!("could not determine cwd: {e}"))?;
    node.share(Arc::new(FullAcpBridge::new(server, agent_id, session_cwd)))
        .await?;

    if !node.wait_online(std::time::Duration::from_secs(15)).await {
        tracing::warn!(
            "roaming endpoint did not come online; the card may lack a reachable address"
        );
    }

    let card = node.card();
    let card_encoded = card.encode()?;
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let status = serde_json::json!({
        "card": card_encoded,
        "endpointId": card.endpoint_id.to_string(),
        "fingerprint": card.fingerprint(),
        "startedAt": started_at,
    });
    if let Some(dir) = status_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp_path = status_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, serde_json::to_vec_pretty(&status)?)?;
    std::fs::rename(&tmp_path, &status_path)?;

    eprintln!("roam is enabled for this server");
    eprintln!("  endpoint id : {}", card.endpoint_id);
    eprintln!("  fingerprint : {}", card.fingerprint());
    eprintln!("your connection card (share with a peer so it can reach you):");
    eprintln!("{card_encoded}");

    Ok(node)
}

async fn handle_serve_command(args: ServeCommandArgs) -> Result<()> {
    use axum::http::HeaderValue;
    use goose::acp::server::AcpBuiltinSelection;
    use goose::acp::server_factory::{AcpServer, AcpServerFactoryConfig};
    use goose::acp::transport::create_router;
    use goose::config::paths::Paths;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tracing::{info, warn};

    let ServeCommandArgs {
        host,
        port,
        tls,
        tls_cert_path,
        tls_key_path,
        platform,
        builtins,
        dangerously_unauthenticated,
        allowed_origins,
        enable_scheduler,
        #[cfg(feature = "roaming")]
        roam,
    } = args;

    let builtins = AcpBuiltinSelection::from_requested(builtins);

    let additional_source_roots = Config::global()
        .get_param::<String>("ADDITIONAL_AGENT_SOURCE_ROOTS")
        .ok()
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let path = path.canonicalize().unwrap_or(path);
            SourceRoot::read_only(path)
        })
        .collect();

    let server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins,
        data_dir: Paths::data_dir(),
        config_dir: Paths::config_dir(),
        goose_platform: platform.into(),
        additional_source_roots,
        session_cwd: None,
        enable_scheduler,
    }));
    let env_secret = std::env::var(GOOSE_SERVER_SECRET_KEY_ENV)
        .ok()
        .map(|secret| secret.trim().to_string())
        .filter(|secret| !secret.is_empty());
    let require_token = env_secret.is_some();
    if !require_token && !dangerously_unauthenticated {
        anyhow::bail!(
            "{GOOSE_SERVER_SECRET_KEY_ENV} must be set to start `goose serve`; pass --dangerously-unauthenticated to run without ACP authentication"
        );
    }
    if dangerously_unauthenticated && !require_token {
        warn!(
            "{GOOSE_SERVER_SECRET_KEY_ENV} is not set and --dangerously-unauthenticated was passed; the ACP endpoint will accept unauthenticated connections"
        );
    }
    let additional_allowed_origins = allowed_origins
        .into_iter()
        .map(|origin| {
            let origin = origin.trim();
            if origin.is_empty() || origin == "*" {
                anyhow::bail!("--allowed-origin must be a non-wildcard Origin value");
            }
            HeaderValue::from_str(origin).map_err(|error| {
                anyhow::anyhow!("invalid --allowed-origin value `{origin}`: {error}")
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let secret_key = env_secret.unwrap_or_else(generate_serve_secret_key);
    if let Err(error) = server.start_scheduler().await {
        warn!("Scheduler failed to start; scheduled jobs will not run until a client connects: {error}");
    }
    #[cfg(feature = "roaming")]
    let roam_share = if roam {
        Some(spawn_roam_share(server.clone()))
    } else {
        None
    };
    let router = create_router(
        server,
        secret_key,
        require_token,
        additional_allowed_origins,
    );

    let config = Config::global();
    let tls_cert_path =
        tls_cert_path.or_else(|| config.get_param::<String>("GOOSE_TLS_CERT_PATH").ok());
    let tls_key_path =
        tls_key_path.or_else(|| config.get_param::<String>("GOOSE_TLS_KEY_PATH").ok());
    let tls = tls
        || config.get_param::<bool>("GOOSE_TLS").unwrap_or(false)
        || tls_cert_path.is_some()
        || tls_key_path.is_some();

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    if tls {
        #[cfg(any(feature = "rustls-tls", feature = "native-tls"))]
        {
            let tls_setup = goose::acp::transport::tls::setup_tls(
                tls_cert_path.as_deref(),
                tls_key_path.as_deref(),
            )
            .await?;
            info!("Starting ACP server on https://{}", addr);

            #[cfg(feature = "rustls-tls")]
            axum_server::bind_rustls(addr, tls_setup.config)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await?;

            #[cfg(feature = "native-tls")]
            axum_server::bind_openssl(addr, tls_setup.config)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await?;
        }

        #[cfg(not(any(feature = "rustls-tls", feature = "native-tls")))]
        {
            let _ = (tls_cert_path, tls_key_path);
            anyhow::bail!(
                "TLS was requested but no TLS backend is enabled. \
                 Enable the `rustls-tls` or `native-tls` feature."
            );
        }
    } else {
        info!("Starting ACP server on http://{}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await?;
    }

    #[cfg(feature = "roaming")]
    if let Some(slot) = roam_share {
        if let Some(node) = slot.write().await.take() {
            let _ = node.shutdown().await;
        }
    }

    Ok(())
}

async fn handle_session_subcommand(command: SessionCommand) -> Result<()> {
    match command {
        SessionCommand::List {
            format,
            ascending,
            working_dir,
            limit,
        } => {
            handle_session_list(format, ascending, working_dir, limit).await?;
        }
        SessionCommand::Remove { identifier, regex } => {
            let (session_id, name) = if let Some(id) = identifier {
                (id.session_id, id.name)
            } else {
                (None, None)
            };
            handle_session_remove(session_id, name, regex).await?;
        }
        SessionCommand::Export {
            identifier,
            output,
            format,
            nostr,
            relays,
        } => {
            let session_manager = SessionManager::instance();
            let session_identifier = if let Some(id) = identifier {
                lookup_session_id(id).await?
            } else {
                match crate::commands::session::prompt_interactive_session_selection(
                    &session_manager,
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        return Ok(());
                    }
                }
            };
            crate::commands::session::handle_session_export(
                session_identifier,
                output,
                format,
                nostr,
                relays,
            )
            .await?;
        }
        SessionCommand::Import { input, nostr } => {
            crate::commands::session::handle_session_import(input, nostr).await?;
        }
        SessionCommand::Diagnostics { identifier, output } => {
            let session_manager = SessionManager::instance();
            let session_id = if let Some(id) = identifier {
                lookup_session_id(id).await?
            } else {
                match crate::commands::session::prompt_interactive_session_selection(
                    &session_manager,
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        return Ok(());
                    }
                }
            };
            crate::commands::session::handle_diagnostics(&session_id, output).await?;
        }
    }
    Ok(())
}

struct InteractiveSessionArgs {
    identifier: Option<Identifier>,
    resume: bool,
    fork: bool,
    edit: bool,
    history: bool,
    system: Option<String>,
    session_opts: SessionOptions,
    extension_opts: ExtensionOptions,
    model_opts: ModelOptions,
}

async fn handle_interactive_session(args: InteractiveSessionArgs) -> Result<()> {
    let InteractiveSessionArgs {
        identifier,
        resume,
        fork,
        edit,
        history,
        system,
        session_opts,
        extension_opts,
        model_opts,
    } = args;
    #[cfg(feature = "telemetry")]
    if get_telemetry_choice().is_none() {
        configure_telemetry_consent_dialog()?;
    }

    let session_start = std::time::Instant::now();
    let session_type = if fork {
        "forked"
    } else if resume {
        "resumed"
    } else {
        "new"
    };

    tracing::info!(
        monotonic_counter.goose.session_starts = 1,
        session_type,
        interactive = true,
        "Session started"
    );

    if let Some(Identifier {
        session_id: Some(_),
        ..
    }) = &identifier
    {
        if !resume {
            eprintln!("Error: --session-id can only be used with --resume flag");
            std::process::exit(1);
        }
    }

    let goose_mode = Config::global().get_goose_mode().unwrap_or_default();
    let mut session_id = get_or_create_session_id(identifier, resume, false, goose_mode).await?;

    if edit || fork {
        if let Some(ref id) = session_id {
            let session_manager = SessionManager::instance();
            let original = session_manager.get_session(id, true).await?;

            let target_id = if fork {
                let copied = session_manager
                    .copy_session(id, original.name.clone())
                    .await?;
                let copied_id = copied.id.clone();
                session_id = Some(copied.id);
                copied_id
            } else {
                id.clone()
            };

            if edit {
                let conversation = original
                    .conversation
                    .ok_or_else(|| anyhow::anyhow!("session has no messages to edit"))?;
                let edited = crate::session::editor::edit_conversation(&conversation)?;
                session_manager
                    .replace_conversation(&target_id, &edited)
                    .await?;
            }
        }
    }

    let mut session: crate::CliSession = build_session(SessionBuilderConfig {
        session_id,
        resume,
        fork,
        no_session: false,
        extensions: extension_opts.extensions,
        streamable_http_extensions: extension_opts.streamable_http_extensions,
        builtins: extension_opts.builtins,
        no_profile: extension_opts.no_profile,
        recipe: None,
        additional_system_prompt: system,
        provider: model_opts.provider,
        model: model_opts.model,
        debug: session_opts.debug,
        max_tool_repetitions: session_opts.max_tool_repetitions,
        max_turns: session_opts.max_turns,
        scheduled_job_id: None,
        interactive: true,
        quiet: false,
        output_format: "text".to_string(),
        container: session_opts.container.map(Container::new),
        stats: false,
    })
    .await;

    if (resume || fork) && history {
        session.render_message_history();
    }

    let result = session.interactive(None).await;
    log_session_completion(&session, session_start, session_type, result.is_ok()).await;
    result
}

async fn log_session_completion(
    session: &crate::CliSession,
    session_start: std::time::Instant,
    session_type: &str,
    success: bool,
) {
    let session_duration = session_start.elapsed();
    let exit_type = if success { "normal" } else { "error" };

    let (total_tokens, message_count) = session
        .get_session()
        .await
        .map(|m| (m.usage.total_tokens.unwrap_or(0), m.message_count))
        .unwrap_or((0, 0));

    tracing::info!(
        monotonic_counter.goose.session_completions = 1,
        session_type,
        exit_type,
        duration_ms = session_duration.as_millis() as u64,
        total_tokens,
        message_count,
        "Session completed"
    );

    tracing::info!(
        monotonic_counter.goose.session_duration_ms = session_duration.as_millis() as u64,
        session_type,
        "Session duration"
    );

    if total_tokens > 0 {
        tracing::info!(
            monotonic_counter.goose.session_tokens = total_tokens,
            session_type,
            "Session tokens"
        );
    }
}

fn parse_run_input(
    input_opts: &InputOptions,
    quiet: bool,
) -> Result<Option<(InputConfig, Option<Recipe>)>> {
    match (
        &input_opts.instructions,
        &input_opts.input_text,
        &input_opts.recipe,
    ) {
        (Some(file), _, _) if file == "-" => {
            let mut contents = String::new();
            std::io::stdin()
                .read_to_string(&mut contents)
                .expect("Failed to read from stdin");
            Ok(Some((
                InputConfig {
                    contents: Some(contents),
                    additional_system_prompt: input_opts.system.clone(),
                },
                None,
            )))
        }
        (Some(file), _, _) => {
            let contents = std::fs::read_to_string(file).unwrap_or_else(|err| {
                eprintln!(
                    "Instruction file not found — did you mean to use goose run --text?\n{}",
                    err
                );
                std::process::exit(1);
            });
            Ok(Some((
                InputConfig {
                    contents: Some(contents),
                    additional_system_prompt: None,
                },
                None,
            )))
        }
        (_, Some(text), _) => Ok(Some((
            InputConfig {
                contents: Some(text.clone()),
                additional_system_prompt: input_opts.system.clone(),
            },
            None,
        ))),
        (_, _, Some(recipe_name)) => {
            let recipe_display_name = std::path::Path::new(recipe_name)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(recipe_name);

            let recipe_version = crate::recipes::search_recipe::load_recipe_file(recipe_name)
                .ok()
                .and_then(|rf| {
                    goose::recipe::template_recipe::parse_recipe_content(
                        &rf.content,
                        Some(rf.parent_dir.display().to_string()),
                    )
                    .ok()
                    .map(|(r, _)| r.version)
                })
                .unwrap_or_else(|| "unknown".to_string());

            if input_opts.explain {
                explain_recipe(recipe_name, input_opts.params.clone())?;
                return Ok(None);
            }
            if input_opts.render_recipe {
                if let Err(err) = render_recipe_as_yaml(recipe_name, input_opts.params.clone()) {
                    eprintln!("{}: {}", console::style("Error").red().bold(), err);
                    std::process::exit(1);
                }
                return Ok(None);
            }

            tracing::info!(
                monotonic_counter.goose.recipe_runs = 1,
                recipe_name = %recipe_display_name,
                recipe_version = %recipe_version,
                session_type = "recipe",
                interface = "cli",
                "Recipe execution started"
            );

            let (input_config, recipe) = extract_recipe_info_from_cli(
                recipe_name.clone(),
                input_opts.params.clone(),
                input_opts.additional_sub_recipes.clone(),
                quiet,
            )?;
            Ok(Some((input_config, Some(recipe))))
        }
        (None, None, None) => {
            eprintln!(
                "Error: Must provide either --instructions (-i), --text (-t), or --recipe. Use -i - for stdin."
            );
            std::process::exit(1);
        }
    }
}

async fn handle_run_command(
    input_opts: InputOptions,
    identifier: Option<Identifier>,
    run_behavior: RunBehavior,
    session_opts: SessionOptions,
    extension_opts: ExtensionOptions,
    output_opts: OutputOptions,
    model_opts: ModelOptions,
) -> Result<()> {
    #[cfg(feature = "telemetry")]
    if run_behavior.interactive && get_telemetry_choice().is_none() {
        configure_telemetry_consent_dialog()?;
    }

    let parsed = parse_run_input(&input_opts, output_opts.quiet)?;

    let Some((input_config, recipe)) = parsed else {
        return Ok(());
    };

    if let Some(Identifier {
        session_id: Some(_),
        ..
    }) = &identifier
    {
        if !run_behavior.resume {
            eprintln!("Error: --session-id can only be used with --resume flag");
            std::process::exit(1);
        }
    }

    let goose_mode = Config::global().get_goose_mode().unwrap_or_default();
    let session_id = get_or_create_session_id(
        identifier,
        run_behavior.resume,
        run_behavior.no_session,
        goose_mode,
    )
    .await?;

    let mut session = build_session(SessionBuilderConfig {
        session_id,
        resume: run_behavior.resume,
        fork: false,
        no_session: run_behavior.no_session,
        extensions: extension_opts.extensions,
        streamable_http_extensions: extension_opts.streamable_http_extensions,
        builtins: extension_opts.builtins,
        no_profile: extension_opts.no_profile,
        recipe: recipe.clone(),
        additional_system_prompt: input_config.additional_system_prompt,
        provider: model_opts.provider,
        model: model_opts.model,
        debug: session_opts.debug,
        max_tool_repetitions: session_opts.max_tool_repetitions,
        max_turns: session_opts.max_turns,
        scheduled_job_id: run_behavior.scheduled_job_id,
        interactive: run_behavior.interactive,
        quiet: output_opts.quiet,
        output_format: output_opts.output_format,
        container: session_opts.container.map(Container::new),
        stats: run_behavior.stats,
    })
    .await;

    if run_behavior.interactive {
        session.interactive(input_config.contents).await
    } else if let Some(contents) = input_config.contents {
        let session_start = std::time::Instant::now();
        let session_type = if recipe.is_some() { "recipe" } else { "run" };

        tracing::info!(
            monotonic_counter.goose.session_starts = 1,
            session_type,
            interactive = false,
            "Headless session started"
        );

        let result = session.headless(contents).await;
        log_session_completion(&session, session_start, session_type, result.is_ok()).await;
        result
    } else {
        Err(anyhow::anyhow!(
            "no text provided for prompt in headless mode"
        ))
    }
}

async fn handle_gateway_command(command: GatewayCommand) -> Result<()> {
    use crate::commands::gateway;

    match command {
        GatewayCommand::Status {} => gateway::handle_gateway_status().await,
        GatewayCommand::Start {
            gateway_type,
            bot_token,
        } => {
            let mut platform_config = serde_json::json!({ "bot_token": bot_token });
            if let Some(ids) = goose::gateway::manager::saved_allowed_user_ids(&gateway_type) {
                platform_config["allowed_user_ids"] = serde_json::json!(ids);
            }
            gateway::handle_gateway_start(gateway_type, platform_config).await
        }
        GatewayCommand::Stop { gateway_type } => gateway::handle_gateway_stop(gateway_type).await,
        GatewayCommand::Pair { gateway_type } => gateway::handle_gateway_pair(gateway_type).await,
    }
}

async fn handle_schedule_command(command: SchedulerCommand) -> Result<()> {
    match command {
        SchedulerCommand::Add {
            schedule_id,
            cron,
            recipe_source,
            params,
        } => handle_schedule_add(schedule_id, cron, recipe_source, params).await,
        SchedulerCommand::List {} => handle_schedule_list().await,
        SchedulerCommand::Remove { schedule_id } => handle_schedule_remove(schedule_id).await,
        SchedulerCommand::Sessions { schedule_id, limit } => {
            handle_schedule_sessions(schedule_id, limit).await
        }
        SchedulerCommand::RunNow { schedule_id } => handle_schedule_run_now(schedule_id).await,
        SchedulerCommand::ServicesStatus {} => handle_schedule_services_status().await,
        SchedulerCommand::ServicesStop {} => handle_schedule_services_stop().await,
        SchedulerCommand::CronHelp {} => handle_schedule_cron_help().await,
    }
}

fn handle_plugin_subcommand(command: PluginCommand) -> Result<()> {
    match command {
        PluginCommand::Install { url, auto_update } => handle_plugin_install(&url, auto_update),
        PluginCommand::Update { name } => handle_plugin_update(&name),
    }
}

fn handle_recipe_subcommand(command: RecipeCommand) -> Result<()> {
    match command {
        RecipeCommand::Validate { recipe_name } => handle_validate(&recipe_name),
        RecipeCommand::Deeplink {
            recipe_name,
            params,
        } => {
            handle_deeplink(&recipe_name, &params)?;
            Ok(())
        }
        RecipeCommand::Open {
            recipe_name,
            params,
        } => handle_open(&recipe_name, &params),
        RecipeCommand::List { format, verbose } => handle_list(&format, verbose),
    }
}

async fn handle_skills_subcommand(command: SkillsCommand) -> Result<()> {
    match command {
        SkillsCommand::List => handle_skills_list().await,
    }
}

async fn handle_term_subcommand(command: TermCommand) -> Result<()> {
    match command {
        TermCommand::Init {
            shell,
            name,
            default,
        } => handle_term_init(shell, name, default).await,
        TermCommand::Log { command } => handle_term_log(command).await,
        TermCommand::Run { prompt } => handle_term_run(prompt).await,
        TermCommand::Info => handle_term_info().await,
    }
}

#[cfg(feature = "local-inference")]
fn print_download_progress(manager: &goose::download_manager::DownloadManager) {
    let Some(progress) = manager
        .list_progress()
        .into_iter()
        .find(|progress| progress.status == goose::download_manager::DownloadStatus::Downloading)
    else {
        return;
    };

    print!(
        "\r  {:.1}% ({:.0}MB / {:.0}MB)",
        progress.progress_percent,
        progress.bytes_downloaded as f64 / (1024.0 * 1024.0),
        progress.total_bytes as f64 / (1024.0 * 1024.0),
    );
    use std::io::Write;
    std::io::stdout().flush().ok();
}

#[cfg(feature = "local-inference")]
fn gb_to_bytes(gb: f64) -> Result<u64> {
    if !gb.is_finite() || gb <= 0.0 {
        anyhow::bail!("--ram-gb must be a positive number");
    }
    Ok((gb * 1024.0 * 1024.0 * 1024.0) as u64)
}

#[cfg(feature = "local-inference")]
fn search_query_from_filters(
    query: Option<String>,
    repo_prefix: Option<&str>,
    repo_suffix: Option<&str>,
) -> String {
    if let Some(query) = query {
        return query;
    }

    if let Some(prefix) = repo_prefix {
        let term = search_term_from_repo_filter(prefix);
        if !term.is_empty() {
            return term;
        }
    }

    if let Some(suffix) = repo_suffix {
        let term = search_term_from_repo_filter(suffix);
        if !term.is_empty() {
            return term;
        }
    }

    String::new()
}

#[cfg(feature = "local-inference")]
fn search_term_from_repo_filter(value: &str) -> String {
    value
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_matches(|c| matches!(c, '-' | '_' | '.'))
        .to_string()
}

#[cfg(feature = "local-inference")]
fn local_search_memory_limit(ram_gb: Option<f64>) -> Result<u64> {
    if let Some(gb) = ram_gb {
        return gb_to_bytes(gb);
    }

    match goose::providers::local_inference::InferenceRuntime::get_or_init() {
        Ok(runtime) => Ok(
            goose::providers::local_inference::available_inference_memory_bytes(runtime.as_ref()),
        ),
        Err(_) => gb_to_bytes(16.0),
    }
}

#[cfg(feature = "local-inference")]
fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        "unknown".to_string()
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(feature = "local-inference")]
fn recommended_variant(
    model: &goose::providers::local_inference::hf_models::HfModelInfo,
    available_memory: u64,
) -> Option<&goose::providers::local_inference::hf_models::HfModelVariant> {
    use goose::providers::local_inference::hf_models::{recommend_variant, HfQuantVariant};

    let mut variant_indexes = Vec::new();
    let mut gguf_variants = Vec::new();
    for (index, variant) in model.variants.iter().enumerate() {
        if variant.backend_id != "llamacpp" || !variant.supported {
            continue;
        }
        variant_indexes.push(index);
        gguf_variants.push(HfQuantVariant {
            quantization: variant.variant_id.clone(),
            size_bytes: variant.size_bytes,
            filename: variant.filename.clone().unwrap_or_default(),
            download_url: variant.download_url.clone().unwrap_or_default(),
            description: "",
            quality_rank: variant.quality_rank,
            sharded: variant.sharded,
        });
    }

    recommend_variant(&gguf_variants, available_memory)
        .map(|index| &model.variants[variant_indexes[index]])
}

#[cfg(feature = "local-inference")]
async fn handle_local_models_command(command: LocalModelsCommand) -> Result<()> {
    use goose::providers::local_inference::hf_models;

    goose::providers::local_inference::configure_huggingface_auth();

    match command {
        LocalModelsCommand::Search {
            query,
            limit,
            repo_prefix,
            repo_suffix,
            quant,
            ram_gb,
            json,
        } => {
            let query =
                search_query_from_filters(query, repo_prefix.as_deref(), repo_suffix.as_deref());
            if !json {
                if query.is_empty() {
                    println!("Searching HuggingFace for local models...");
                } else {
                    println!("Searching HuggingFace for '{}'...", query);
                }
            }
            let has_local_filter =
                repo_prefix.is_some() || repo_suffix.is_some() || quant.is_some();
            let fetch_limit = if has_local_filter {
                limit.saturating_mul(5).max(limit).max(50)
            } else {
                limit
            };
            let mut results = hf_models::search_local_models(&query, fetch_limit).await?;
            let quant = quant.map(|value| value.to_lowercase());
            results.retain_mut(|model| {
                if repo_prefix
                    .as_deref()
                    .is_some_and(|prefix| !model.repo_id.starts_with(prefix))
                {
                    return false;
                }
                if repo_suffix
                    .as_deref()
                    .is_some_and(|suffix| !model.repo_id.ends_with(suffix))
                {
                    return false;
                }

                if let Some(quant) = &quant {
                    model
                        .variants
                        .retain(|variant| variant.variant_id.to_lowercase().contains(quant));
                }

                !model.variants.is_empty()
            });
            results.truncate(limit);

            if results.is_empty() {
                if json {
                    println!("[]");
                } else {
                    println!("No compatible local models found.");
                }
                return Ok(());
            }

            let available_memory = local_search_memory_limit(ram_gb)?;

            if json {
                let output = results
                    .iter()
                    .map(|model| {
                        let recommended_variant =
                            recommended_variant(model, available_memory).map(|variant| {
                                serde_json::json!({
                                    "model_id": variant.model_id,
                                    "download_id": variant.download_id,
                                    "label": variant.label,
                                    "size_bytes": variant.size_bytes,
                                })
                            });
                        serde_json::json!({
                            "repo_id": model.repo_id,
                            "author": model.author,
                            "model_name": model.model_name,
                            "downloads": model.downloads,
                            "recommended_variant": recommended_variant,
                        })
                    })
                    .collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Ok(());
            }

            for model in &results {
                println!(
                    "\n{} (by {}) — {} downloads",
                    model.model_name, model.author, model.downloads
                );
                if let Some(variant) = recommended_variant(model, available_memory) {
                    println!(
                        "  Recommended: {} — {}",
                        variant.label,
                        format_size(variant.size_bytes)
                    );
                    println!(
                        "    Download: goose local-models download '{}'",
                        variant.download_id
                    );
                } else {
                    println!(
                        "  Recommended: none fits in {}",
                        format_size(available_memory)
                    );
                }
                for variant in &model.variants {
                    let support = if variant.supported {
                        String::new()
                    } else {
                        format!(
                            " ({})",
                            variant
                                .unsupported_reason
                                .as_deref()
                                .unwrap_or("unsupported on this platform")
                        )
                    };
                    println!(
                        "  [{}] {} — {} — {}{}",
                        variant.format,
                        variant.label,
                        format_size(variant.size_bytes),
                        variant.description,
                        support
                    );
                    if variant.supported {
                        println!(
                            "    Download: goose local-models download '{}'",
                            variant.download_id
                        );
                    }
                }
            }
        }
        LocalModelsCommand::Download { spec } => {
            println!("Resolving {}...", spec);
            let manager = goose::download_manager::get_download_manager();
            let resolve_task = hf_models::resolve_local_model_spec(&spec);
            tokio::pin!(resolve_task);
            let resolved = loop {
                tokio::select! {
                    result = &mut resolve_task => break result?,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                        print_download_progress(manager);
                    }
                }
            };
            let model_id = resolved.model_id();
            let total_size = resolved.total_size();

            println!(
                "\nDownloaded {} ({})",
                model_id,
                if total_size > 0 {
                    format!("{:.1}GB", total_size as f64 / (1024.0 * 1024.0 * 1024.0))
                } else {
                    "unknown size".to_string()
                }
            );
        }
        LocalModelsCommand::List => {
            let models = hf_models::cached_local_models().await?;

            if models.is_empty() {
                println!("No local models downloaded.");
                return Ok(());
            }

            println!(
                "{:<50} {:<10} {:<12} Downloaded",
                "ID", "Backend", "Variant"
            );
            println!("{}", "-".repeat(88));
            for m in &models {
                println!("{:<50} {:<10} {:<12} ✓", m.id, m.backend_id, m.quantization);
            }
        }
        LocalModelsCommand::Delete { id } => {
            if hf_models::cached_local_model(&id).await?.is_some() {
                hf_models::delete_cached_local_model(&id).await?;
                println!("Deleted model: {}", id);
            } else {
                println!("Model not found: {}", id);
            }
        }
    }

    Ok(())
}

async fn handle_default_session() -> Result<()> {
    if !Config::global().exists() {
        return handle_configure().await;
    }

    #[cfg(feature = "telemetry")]
    if get_telemetry_choice().is_none() {
        configure_telemetry_consent_dialog()?;
    }

    let goose_mode = Config::global().get_goose_mode().unwrap_or_default();
    let session_id = get_or_create_session_id(None, false, false, goose_mode).await?;

    let mut session = build_session(SessionBuilderConfig {
        session_id,
        resume: false,
        fork: false,
        no_session: false,
        extensions: Vec::new(),
        streamable_http_extensions: Vec::new(),
        builtins: Vec::new(),
        no_profile: false,
        recipe: None,
        additional_system_prompt: None,
        provider: None,
        model: None,
        debug: false,
        max_tool_repetitions: None,
        max_turns: None,
        scheduled_job_id: None,
        interactive: true,
        quiet: false,
        output_format: "text".to_string(),
        container: None,
        stats: false,
    })
    .await;
    session.interactive(None).await
}

pub async fn cli() -> anyhow::Result<()> {
    register_builtin_extensions(goose_mcp::BUILTIN_EXTENSIONS.clone());

    let cli = Cli::parse();

    let command_name = get_command_name(&cli.command);
    tracing::info!(
        monotonic_counter.goose.cli_commands = 1,
        command = command_name,
        "CLI command executed"
    );

    match cli.command {
        Some(Command::Completion { shell, bin_name }) => {
            let mut cmd = Cli::command();
            shell.generate(&mut cmd, &bin_name, &mut std::io::stdout());
            Ok(())
        }
        Some(Command::Configure {}) => handle_configure().await,
        Some(Command::Doctor {}) => crate::commands::doctor::handle_doctor().await,
        Some(Command::Info { verbose, check }) => handle_info(verbose, check).await,
        Some(Command::Mcp { server }) => handle_mcp_command(server).await,
        Some(Command::Acp {
            builtins,
            enable_scheduler,
        }) => goose::acp::server::run(builtins, enable_scheduler).await,
        #[cfg(feature = "roaming")]
        Some(Command::Roam { command }) => handle_roam_command(command).await,
        Some(Command::Serve {
            host,
            port,
            tls,
            tls_cert_path,
            tls_key_path,
            platform,
            builtins,
            dangerously_unauthenticated,
            allowed_origins,
            enable_scheduler,
            #[cfg(feature = "roaming")]
            roam,
        }) => {
            handle_serve_command(ServeCommandArgs {
                host,
                port,
                tls,
                tls_cert_path,
                tls_key_path,
                platform,
                builtins,
                dangerously_unauthenticated,
                allowed_origins,
                enable_scheduler,
                #[cfg(feature = "roaming")]
                roam,
            })
            .await
        }
        Some(Command::Session {
            command: Some(cmd), ..
        }) => handle_session_subcommand(cmd).await,
        Some(Command::Session {
            command: None,
            identifier,
            resume,
            fork,
            edit,
            history,
            system,
            session_opts,
            extension_opts,
            model_opts,
        }) => {
            handle_interactive_session(InteractiveSessionArgs {
                identifier,
                resume,
                fork,
                edit,
                history,
                system,
                session_opts,
                extension_opts,
                model_opts,
            })
            .await
        }
        Some(Command::Run {
            input_opts,
            identifier,
            run_behavior,
            session_opts,
            extension_opts,
            output_opts,
            model_opts,
        }) => {
            handle_run_command(
                input_opts,
                identifier,
                run_behavior,
                session_opts,
                extension_opts,
                output_opts,
                model_opts,
            )
            .await
        }
        Some(Command::Gateway { command }) => handle_gateway_command(command).await,
        Some(Command::Schedule { command }) => handle_schedule_command(command).await,
        #[cfg(feature = "update")]
        Some(Command::Update {
            canary,
            reconfigure,
        }) => {
            crate::commands::update::update(canary, reconfigure).await?;
            Ok(())
        }
        Some(Command::Recipe { command }) => handle_recipe_subcommand(command),
        Some(Command::Skills { command }) => handle_skills_subcommand(command).await,
        Some(Command::Plugin { command }) => handle_plugin_subcommand(command),
        Some(Command::Term { command }) => handle_term_subcommand(command).await,
        #[cfg(feature = "local-inference")]
        Some(Command::LocalModels { command }) => handle_local_models_command(command).await,
        Some(Command::Review {
            range,
            prompt,
            model,
            provider,
            override_model,
            turn_limit,
            dry_run,
            quiet,
            no_orchestrate,
            instructions,
            files,
            check_filter,
            check_scope,
            checks_only,
            summary_only,
            severity,
        }) => {
            use crate::commands::review::{handle_review, ReviewOptions};
            handle_review(ReviewOptions {
                range,
                prompt_file: prompt,
                default_model: model,
                provider,
                override_model,
                default_turn_limit: turn_limit,
                dry_run,
                quiet,
                no_orchestrate,
                instructions,
                files,
                check_filter,
                check_scope,
                checks_only,
                summary_only,
                severity,
            })
            .await
        }
        Some(Command::ValidateExtensions { file }) => {
            use goose::agents::validate_extensions::validate_bundled_extensions;
            match validate_bundled_extensions(&file) {
                Ok(msg) => {
                    println!("{msg}");
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some(Command::McpProbe { extension, script }) => handle_mcp_probe(extension, script).await,
        None => handle_default_session().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_command_accepts_nushell_alias() {
        let cli = Cli::try_parse_from(["goose", "completion", "nushell"]).expect("parse failed");

        match cli.command {
            Some(Command::Completion {
                shell: CompletionShell::Nu,
                ..
            }) => {}
            _ => panic!("expected nu completion shell"),
        }
    }

    #[test]
    fn session_resume_accepts_provider_and_model_overrides() {
        let cli = Cli::try_parse_from([
            "goose",
            "session",
            "--resume",
            "--provider",
            "openai",
            "--model",
            "gpt-5.4",
        ])
        .expect("parse failed");

        match cli.command {
            Some(Command::Session {
                resume, model_opts, ..
            }) => {
                assert!(resume);
                assert_eq!(model_opts.provider.as_deref(), Some("openai"));
                assert_eq!(model_opts.model.as_deref(), Some("gpt-5.4"));
            }
            _ => panic!("expected session command"),
        }
    }

    #[test]
    fn session_accepts_provider_override_without_resume() {
        let cli = Cli::try_parse_from(["goose", "session", "--provider", "openai"])
            .expect("provider override should work for a new session");

        match cli.command {
            Some(Command::Session {
                resume, model_opts, ..
            }) => {
                assert!(!resume);
                assert_eq!(model_opts.provider.as_deref(), Some("openai"));
            }
            _ => panic!("expected session command"),
        }
    }

    #[test]
    fn session_accepts_model_override_without_resume() {
        let cli = Cli::try_parse_from(["goose", "session", "--model", "gpt-5.4"])
            .expect("model override should work for a new session");

        match cli.command {
            Some(Command::Session {
                resume, model_opts, ..
            }) => {
                assert!(!resume);
                assert_eq!(model_opts.model.as_deref(), Some("gpt-5.4"));
            }
            _ => panic!("expected session command"),
        }
    }

    #[test]
    fn session_accepts_system_prompt() {
        let cli = Cli::try_parse_from(["goose", "session", "--system", "extra instructions"])
            .expect("system prompt should work for a new session");

        match cli.command {
            Some(Command::Session { system, .. }) => {
                assert_eq!(system.as_deref(), Some("extra instructions"));
            }
            _ => panic!("expected session command"),
        }
    }

    #[test]
    fn nushell_completion_generation_emits_module() {
        let mut cmd = Cli::command();
        let mut buffer = Vec::new();

        CompletionShell::Nu.generate(&mut cmd, "goose", &mut buffer);

        let script = String::from_utf8(buffer).expect("utf8");
        assert!(script.contains("module completions"));
        assert!(script.contains("export extern goose"));
        assert!(script.contains("export use completions *"));
    }

    #[test]
    fn term_init_help_mentions_nushell() {
        let mut cmd = Cli::command();
        let term = cmd.find_subcommand_mut("term").expect("term command");
        let init = term.find_subcommand_mut("init").expect("init command");
        let mut buffer = Vec::new();

        init.write_long_help(&mut buffer).expect("write help");

        let help = String::from_utf8(buffer).expect("utf8");
        assert!(help.contains("goose term init nu"));
        assert!(help.contains("Supported for zsh, bash, and nu"));
    }

    #[test]
    fn completion_help_lists_nu() {
        let mut cmd = Cli::command();
        let completion = cmd
            .find_subcommand_mut("completion")
            .expect("completion command");
        let mut buffer = Vec::new();

        completion.write_long_help(&mut buffer).expect("write help");

        let help = String::from_utf8(buffer).expect("utf8");
        assert!(help.contains("nu"));
    }

    #[test]
    fn skills_command_accepts_list_subcommand() {
        let cli = Cli::try_parse_from(["goose", "skills", "list"]).expect("parse failed");

        match cli.command {
            Some(Command::Skills {
                command: SkillsCommand::List,
            }) => {}
            _ => panic!("expected skills list command"),
        }
    }

    #[test]
    fn serve_command_accepts_dangerously_unauthenticated_flag() {
        let cli = Cli::try_parse_from([
            "goose",
            "serve",
            "--dangerously-unauthenticated",
            "--allowed-origin",
            "app://localhost",
            "--allowed-origin",
            "https://app.example",
        ])
        .expect("parse failed");

        match cli.command {
            Some(Command::Serve {
                dangerously_unauthenticated,
                allowed_origins,
                ..
            }) => {
                assert!(dangerously_unauthenticated);
                assert_eq!(
                    allowed_origins,
                    vec!["app://localhost", "https://app.example"]
                );
            }
            _ => panic!("expected serve command"),
        }
    }

    #[test]
    fn review_command_accepts_options() {
        let cli = Cli::try_parse_from([
            "goose",
            "review",
            "origin/main...HEAD",
            "--prompt",
            "REVIEW.md",
            "--model",
            "test-model",
            "--provider",
            "openai",
            "--override-model",
            "check-model",
            "--turn-limit",
            "4",
            "--dry-run",
            "--quiet",
            "--no-orchestrate",
            "--instructions",
            "focus on correctness",
            "--files",
            "src/lib.rs",
            "--check-filter",
            "security",
            "--check-scope",
            ".agents",
            "--checks-only",
            "--summary-only",
            "--severity",
            "low",
        ])
        .expect("parse failed");

        match cli.command {
            Some(Command::Review {
                range,
                prompt,
                model,
                provider,
                override_model,
                turn_limit,
                dry_run,
                quiet,
                no_orchestrate,
                instructions,
                files,
                check_filter,
                check_scope,
                checks_only,
                summary_only,
                severity,
            }) => {
                assert_eq!(range.as_deref(), Some("origin/main...HEAD"));
                assert_eq!(prompt.as_deref(), Some(std::path::Path::new("REVIEW.md")));
                assert_eq!(model.as_deref(), Some("test-model"));
                assert_eq!(provider.as_deref(), Some("openai"));
                assert_eq!(override_model.as_deref(), Some("check-model"));
                assert_eq!(turn_limit, Some(4));
                assert!(dry_run);
                assert!(quiet);
                assert!(no_orchestrate);
                assert_eq!(instructions.as_deref(), Some("focus on correctness"));
                assert_eq!(files, vec!["src/lib.rs"]);
                assert_eq!(check_filter, vec!["security"]);
                assert_eq!(
                    check_scope.as_deref(),
                    Some(std::path::Path::new(".agents"))
                );
                assert!(checks_only);
                assert!(summary_only);
                assert_eq!(severity, "low");
            }
            _ => panic!("expected review command"),
        }
    }

    #[cfg(feature = "local-inference")]
    mod local_search {
        use super::super::{
            format_size, gb_to_bytes, search_query_from_filters, search_term_from_repo_filter,
        };

        #[test]
        fn gb_to_bytes_converts_and_rejects_nonpositive() {
            assert_eq!(gb_to_bytes(1.0).unwrap(), 1024 * 1024 * 1024);
            assert_eq!(gb_to_bytes(0.5).unwrap(), 512 * 1024 * 1024);
            assert!(gb_to_bytes(0.0).is_err());
            assert!(gb_to_bytes(-4.0).is_err());
            assert!(gb_to_bytes(f64::NAN).is_err());
            assert!(gb_to_bytes(f64::INFINITY).is_err());
        }

        #[test]
        fn explicit_query_wins_over_repo_filters() {
            let query = search_query_from_filters(
                Some("qwen".to_string()),
                Some("unsloth/Llama-3.2"),
                Some("-GGUF"),
            );
            assert_eq!(query, "qwen");
        }

        #[test]
        fn repo_prefix_is_used_when_query_is_absent() {
            let query = search_query_from_filters(None, Some("unsloth/Llama-3.2"), None);
            assert_eq!(query, "Llama-3.2");
        }

        #[test]
        fn repo_suffix_is_used_when_prefix_yields_nothing() {
            let query = search_query_from_filters(None, Some("///"), Some("-Qwen3-GGUF"));
            assert_eq!(query, "Qwen3-GGUF");
        }

        #[test]
        fn empty_when_nothing_is_provided() {
            assert_eq!(search_query_from_filters(None, None, None), "");
        }

        #[test]
        fn repo_filter_takes_last_path_segment_and_trims_separators() {
            assert_eq!(
                search_term_from_repo_filter("unsloth/Llama-3.2"),
                "Llama-3.2"
            );
            assert_eq!(search_term_from_repo_filter("-GGUF"), "GGUF");
            assert_eq!(search_term_from_repo_filter("/bartowski/"), "bartowski");
            assert_eq!(search_term_from_repo_filter("_model_."), "model");
            assert_eq!(search_term_from_repo_filter(""), "");
        }

        #[test]
        fn format_size_reports_unknown_for_zero() {
            assert_eq!(format_size(0), "unknown");
            assert_eq!(format_size(1024 * 1024 * 1024), "1.0GB");
            assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.5GB");
        }
    }
}
