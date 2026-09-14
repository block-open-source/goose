use crate::cli::StreamableHttpOptions;

use super::output;
use super::{derive_extension_name_from_command, split_extension_name_prefix, CliSession};
use console::style;
use goose::agents::{Agent, Container, ExtensionError};
use goose::config::extensions::name_to_key;
use goose::config::resolve_extensions_for_new_session;
use goose::config::{Config, ExtensionConfig, GooseMode};
use goose::model_config::model_config_from_user_config;
use goose::providers::create;
use goose::recipe::Recipe;
use goose::session::session_manager::SessionType;
use goose::session::EnabledExtensionsState;
use rustyline::EditMode;
use std::collections::{HashMap, HashSet};
use std::process;
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;

const EXTENSION_HINT_MAX_LEN: usize = 5;

fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    let truncated: String = s.chars().take(max_len).collect();
    if s.chars().count() > max_len {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

// Plain String rather than `ExtensionError`: the only variant this function
// ever constructs is `ConfigError(String)`, but clippy's `result_large_err`
// sizes an error type by its largest variant, and `ExtensionError` carries a
// `ClientError`/`ClientInitializeError` far past the 128-byte default
// threshold. Callers wrap this back into `ExtensionError::ConfigError`.
fn disambiguate_stdio_extension_names(
    extensions: &mut [(String, ExtensionConfig)],
    renameable: &HashSet<usize>,
) -> Result<(), String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    // Only entries the caller marked fixed (explicitly named, or not a CLI
    // stdio extension at all) can make this an error — a fixed name colliding
    // with a renameable one is exactly the case the loop below resolves by
    // renaming the renameable side, not a real conflict.
    let mut fixed_counts: HashMap<String, usize> = HashMap::new();
    for (idx, (_, config)) in extensions.iter().enumerate() {
        let key = config.key();
        *counts.entry(key.clone()).or_default() += 1;
        if !renameable.contains(&idx) {
            *fixed_counts.entry(key).or_default() += 1;
        }
    }

    let duplicate_fixed_names = extensions.iter().enumerate().find(|(index, (_, config))| {
        !renameable.contains(index) && fixed_counts.get(&config.key()).copied().unwrap_or(0) > 1
    });
    if let Some((_, (_, config))) = duplicate_fixed_names {
        return Err(format!(
            "extension name '{}' is already in use",
            config.name()
        ));
    }

    let mut taken: HashSet<String> = counts.keys().cloned().collect();
    for (idx, (_, config)) in extensions.iter_mut().enumerate() {
        if !renameable.contains(&idx) || counts.get(&config.key()).copied().unwrap_or(0) < 2 {
            continue;
        }
        let ExtensionConfig::Stdio {
            name, cmd, args, ..
        } = config
        else {
            continue;
        };
        let derived = derive_extension_name_from_command(cmd, args);
        let mut candidate = derived.clone();
        let mut suffix = 2;
        while candidate.is_empty() || taken.contains(&name_to_key(&candidate)) {
            candidate = format!("{}_{}", derived, suffix);
            suffix += 1;
        }
        taken.insert(name_to_key(&candidate));
        *name = candidate;
    }
    Ok(())
}

fn is_builtin_or_platform_extension(config: &ExtensionConfig) -> bool {
    matches!(
        config,
        ExtensionConfig::Builtin { .. } | ExtensionConfig::Platform { .. }
    )
}

fn deduplicate_cli_builtins(
    existing: &[(String, ExtensionConfig)],
    cli_extensions: &mut Vec<(String, ExtensionConfig, bool)>,
) {
    let mut seen_builtin_names = existing
        .iter()
        .filter(|(_, config)| is_builtin_or_platform_extension(config))
        .map(|(_, config)| config.key())
        .collect::<HashSet<_>>();

    cli_extensions.retain(|(_, config, _)| {
        !is_builtin_or_platform_extension(config) || seen_builtin_names.insert(config.key())
    });
}

fn parse_cli_flag_extensions(
    extensions: &[String],
    streamable_http_extensions: &[StreamableHttpOptions],
    builtins: &[String],
) -> Vec<(String, ExtensionConfig, bool)> {
    let mut extensions_to_load = Vec::new();

    for (idx, ext_str) in extensions.iter().enumerate() {
        match CliSession::parse_stdio_extension(ext_str) {
            Ok(config) => {
                let hint = truncate_with_ellipsis(ext_str, EXTENSION_HINT_MAX_LEN);
                let label = format!("stdio #{}({})", idx + 1, hint);
                let explicitly_named = split_extension_name_prefix(ext_str).0.is_some();
                extensions_to_load.push((label, config, !explicitly_named));
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    style(format!(
                        "Warning: Invalid --extension value '{}' ({}); ignoring",
                        ext_str, e
                    ))
                    .yellow()
                );
            }
        }
    }

    for (idx, opts) in streamable_http_extensions.iter().enumerate() {
        let config = CliSession::parse_streamable_http_extension(&opts.url, opts.timeout);
        let hint = truncate_with_ellipsis(&opts.url, EXTENSION_HINT_MAX_LEN);
        let label = format!("http #{}({})", idx + 1, hint);
        extensions_to_load.push((label, config, false));
    }

    for builtin_str in builtins {
        let configs = CliSession::parse_builtin_extensions(builtin_str);
        for config in configs {
            extensions_to_load.push((config.name(), config, false));
        }
    }

    extensions_to_load
}

/// Configuration for building a new Goose session
///
/// This struct contains all the parameters needed to create a new session,
/// including session identification, extension configuration, and debug settings.
#[derive(Clone, Debug)]
pub struct SessionBuilderConfig {
    /// Session id, optional need to deduce from context
    pub session_id: Option<String>,
    /// Whether to resume an existing session
    pub resume: bool,
    /// Whether to fork an existing session (creates a copy of the original/existing session then resumes the copy)
    pub fork: bool,
    /// Whether to run without a session file
    pub no_session: bool,
    /// List of stdio extension commands to add
    pub extensions: Vec<String>,
    /// List of streamable HTTP extension commands to add
    pub streamable_http_extensions: Vec<StreamableHttpOptions>,
    /// List of builtin extension commands to add
    pub builtins: Vec<String>,
    pub no_profile: bool,
    /// Recipe for the session
    pub recipe: Option<Recipe>,
    /// Any additional system prompt to append to the default
    pub additional_system_prompt: Option<String>,
    /// Provider override from CLI arguments
    pub provider: Option<String>,
    /// Model override from CLI arguments
    pub model: Option<String>,
    /// Enable debug printing
    pub debug: bool,
    /// Maximum number of consecutive identical tool calls allowed
    pub max_tool_repetitions: Option<u32>,
    /// Maximum number of turns (iterations) allowed without user input
    pub max_turns: Option<u32>,
    /// ID of the scheduled job that triggered this session (if any)
    pub scheduled_job_id: Option<String>,
    /// Whether this session will be used interactively (affects debugging prompts)
    pub interactive: bool,
    /// Quiet mode - suppress non-response output
    pub quiet: bool,
    /// Output format (text, json)
    pub output_format: String,
    /// Docker container to run stdio extensions inside
    pub container: Option<Container>,
    /// Print generation statistics after headless runs.
    pub stats: bool,
}

/// Manual implementation of Default to ensure proper initialization of output_format
/// This struct requires explicit default value for output_format field
impl Default for SessionBuilderConfig {
    fn default() -> Self {
        SessionBuilderConfig {
            session_id: None,
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
            interactive: false,
            quiet: false,
            output_format: "text".to_string(),
            container: None,
            stats: false,
        }
    }
}

pub struct ExtensionFailure {
    pub label: Option<String>,
    pub error: anyhow::Error,
}

async fn load_extensions(
    agent: Arc<Agent>,
    extensions: Vec<ExtensionConfig>,
    session_id: &str,
) -> Vec<ExtensionFailure> {
    let results = match agent.add_extensions_bulk(extensions, session_id).await {
        Ok(results) => results,
        Err(error) => {
            tracing::error!("failed to load extensions: {}", error);
            return vec![ExtensionFailure { label: None, error }];
        }
    };

    results
        .into_iter()
        .filter_map(|r| {
            r.error.map(|error| ExtensionFailure {
                label: Some(r.name),
                error: anyhow::anyhow!(error),
            })
        })
        .collect()
}

struct ResolvedProviderConfig {
    provider_name: String,
    model_name: String,
    model_config: goose_providers::model::ModelConfig,
}

fn validate_provider_override_context(
    session_config: &SessionBuilderConfig,
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
    provider_name: &str,
    model_name: &str,
    provider_manages_own_context: bool,
) -> anyhow::Result<()> {
    let provider_changed = saved_provider
        .map(|saved| saved != provider_name)
        .unwrap_or_else(|| session_config.provider.is_some());
    let model_changed = saved_model
        .map(|saved| saved != model_name)
        .unwrap_or_else(|| session_config.model.is_some());

    if session_config.resume && provider_manages_own_context && (provider_changed || model_changed)
    {
        anyhow::bail!(
            "Cannot resume with provider or model changes because provider '{}' manages its own conversation context. Start a new session to use this provider or model.",
            provider_name
        );
    }

    Ok(())
}

async fn resolve_provider_and_model(
    session_config: &SessionBuilderConfig,
    config: &Config,
    saved_provider: Option<String>,
    saved_model_config: Option<goose_providers::model::ModelConfig>,
) -> ResolvedProviderConfig {
    let recipe_settings = session_config
        .recipe
        .as_ref()
        .and_then(|r| r.settings.as_ref());
    let configured_provider = config.get_goose_provider().ok();

    let provider_name = session_config
        .provider
        .clone()
        .or_else(|| saved_provider.clone())
        .or_else(|| recipe_settings.and_then(|s| s.goose_provider.clone()))
        .or_else(|| configured_provider.clone())
        .unwrap_or_else(|| {
            output::render_error("No provider configured. Run 'goose configure' first.");
            process::exit(1);
        });

    let saved_provider_matches = saved_provider.as_deref() == Some(provider_name.as_str());
    let provider_overridden = session_config.provider.is_some();
    let matching_recipe_model = recipe_settings.and_then(|settings| {
        let recipe_provider_matches = settings
            .goose_provider
            .as_deref()
            .is_none_or(|provider| provider == provider_name);

        if provider_overridden && recipe_provider_matches {
            settings.goose_model.clone()
        } else {
            None
        }
    });
    let matching_environment_model =
        if provider_overridden && configured_provider.as_deref() == Some(provider_name.as_str()) {
            std::env::var("GOOSE_MODEL").ok()
        } else {
            None
        };
    let matching_config_model =
        if provider_overridden && configured_provider.as_deref() == Some(provider_name.as_str()) {
            config.get_goose_model().ok()
        } else {
            None
        };
    let configured_provider_model = session_config.provider.as_ref().and_then(|_| {
        goose::config::get_provider_entry(config, &provider_name)
            .map(|entry| entry.model)
            .filter(|model| !model.is_empty())
    });
    let target_provider_default = if provider_overridden
        && session_config.model.is_none()
        && matching_recipe_model.is_none()
        && matching_environment_model.is_none()
        && matching_config_model.is_none()
        && configured_provider_model.is_none()
    {
        Some(
            goose::providers::get_from_registry(&provider_name)
                .await
                .unwrap_or_else(|e| {
                    output::render_error(&e.to_string());
                    process::exit(1);
                })
                .metadata()
                .default_model
                .clone(),
        )
        .filter(|model| !model.is_empty())
    } else {
        None
    };

    let model_name = session_config
        .model
        .clone()
        .or_else(|| {
            if session_config.resume {
                matching_environment_model.clone()
            } else {
                None
            }
        })
        .or_else(|| {
            if saved_provider_matches {
                saved_model_config.as_ref().map(|mc| mc.model_name.clone())
            } else {
                None
            }
        })
        .or(matching_recipe_model)
        .or(matching_environment_model)
        .or(matching_config_model)
        .or(configured_provider_model)
        .or(target_provider_default)
        .or_else(|| {
            if provider_overridden {
                None
            } else {
                recipe_settings.and_then(|s| s.goose_model.clone())
            }
        })
        .or_else(|| {
            if provider_overridden {
                None
            } else {
                config.get_goose_model().ok()
            }
        })
        .unwrap_or_else(|| {
            output::render_error("No model configured. Run 'goose configure' first.");
            process::exit(1);
        });

    let mut model_config = if session_config.resume
        && saved_provider_matches
        && saved_model_config
            .as_ref()
            .is_some_and(|mc| mc.model_name == model_name)
    {
        let mut config = saved_model_config.unwrap();
        config.normalize_effort_suffix();
        config = goose::model_config::with_rederived_cache_ttl(config).unwrap_or_else(|e| {
            output::render_error(&format!("Invalid cache TTL configuration: {}", e));
            process::exit(1);
        });
        if let Some(temp) = recipe_settings.and_then(|s| s.temperature) {
            config = config.with_temperature(Some(temp));
        }
        config
    } else {
        let mut config =
            goose::model_config::model_config_from_user_config(&provider_name, &model_name)
                .unwrap_or_else(|e| {
                    output::render_error(&format!("Failed to create model configuration: {}", e));
                    process::exit(1);
                });
        if let Some(temp) = recipe_settings.and_then(|s| s.temperature) {
            config = config.with_temperature(Some(temp));
        }
        config
    };

    if !session_config.interactive {
        model_config = model_config.with_cache_ttl_clamped();
    }

    ResolvedProviderConfig {
        provider_name,
        model_name,
        model_config,
    }
}

async fn resolve_session_id(
    session_config: &SessionBuilderConfig,
    session_manager: &goose::session::session_manager::SessionManager,
    goose_mode: GooseMode,
) -> String {
    if session_config.no_session {
        let working_dir = std::env::current_dir().unwrap_or_else(|e| {
            output::render_error(&format!("Could not get working directory: {}", e));
            process::exit(1);
        });
        let session = session_manager
            .create_session(
                working_dir,
                "CLI Session".to_string(),
                SessionType::Hidden,
                goose_mode,
            )
            .await
            .unwrap_or_else(|e| {
                output::render_error(&format!("Could not create session: {}", e));
                process::exit(1);
            });
        session.id
    } else if session_config.resume {
        if let Some(ref session_id) = session_config.session_id {
            match session_manager.get_session(session_id, false).await {
                Ok(_) => session_id.clone(),
                Err(_) => {
                    output::render_error(&format!(
                        "Cannot resume session {} - no such session exists",
                        style(session_id).cyan()
                    ));
                    process::exit(1);
                }
            }
        } else {
            match session_manager
                .list_sessions_by_types(&[SessionType::User])
                .await
            {
                Ok(sessions) if !sessions.is_empty() => sessions[0].id.clone(),
                _ => {
                    output::render_error("Cannot resume - no previous sessions found");
                    process::exit(1);
                }
            }
        }
    } else {
        session_config.session_id.clone().unwrap()
    }
}

async fn handle_resumed_session_workdir(agent: &Agent, session_id: &str, interactive: bool) {
    let session = agent
        .config
        .session_manager
        .get_session(session_id, false)
        .await
        .unwrap_or_else(|e| {
            output::render_error(&format!("Failed to read session metadata: {}", e));
            process::exit(1);
        });

    let current_workdir = std::env::current_dir().unwrap_or_else(|e| {
        output::render_error(&format!("Failed to get current working directory: {}", e));
        process::exit(1);
    });
    if current_workdir == session.working_dir {
        return;
    }

    if interactive {
        let change_workdir = cliclack::confirm(format!(
            "{} The original working directory of this session was set to {}. \
             Your current directory is {}. \
             Do you want to switch back to the original working directory?",
            style("WARNING:").yellow(),
            style(session.working_dir.display()).cyan(),
            style(current_workdir.display()).cyan(),
        ))
        .initial_value(true)
        .interact()
        .unwrap_or_else(|e| {
            output::render_error(&format!("Failed to get user input: {}", e));
            process::exit(1);
        });

        if change_workdir {
            if !session.working_dir.exists() {
                output::render_error(&format!(
                    "Cannot switch to original working directory - {} no longer exists",
                    style(session.working_dir.display()).cyan()
                ));
            } else if let Err(e) = std::env::set_current_dir(&session.working_dir) {
                output::render_error(&format!(
                    "Failed to switch to original working directory: {}",
                    e
                ));
            }
        }
    } else {
        eprintln!(
            "{}",
            style(format!(
                "Warning: Working directory differs from session (current: {}, session: {}). \
                 Staying in current directory.",
                current_workdir.display(),
                session.working_dir.display()
            ))
            .yellow()
        );
    }
}

async fn collect_extension_configs(
    agent: &Agent,
    session_config: &SessionBuilderConfig,
    recipe: Option<&Recipe>,
    session_id: &str,
) -> Result<Vec<ExtensionConfig>, ExtensionError> {
    let recipe_extensions = recipe.and_then(|r| r.extensions.as_deref());
    let configured_extensions: Vec<ExtensionConfig> = if session_config.resume {
        EnabledExtensionsState::for_session(
            &agent.config.session_manager,
            session_id,
            Config::global(),
        )
        .await
    } else if session_config.no_profile {
        Vec::new()
    } else {
        resolve_extensions_for_new_session(recipe_extensions, None)
    };

    let mut cli_flag_extensions = parse_cli_flag_extensions(
        &session_config.extensions,
        &session_config.streamable_http_extensions,
        &session_config.builtins,
    );

    let mut all: Vec<(String, ExtensionConfig)> = configured_extensions
        .into_iter()
        .map(|config| (config.name(), config))
        .collect();
    if !session_config.no_profile && !session_config.resume && recipe_extensions.is_none() {
        let project_root = std::env::current_dir().ok();
        all.extend(
            goose::plugins::mcp_servers::enabled_plugin_mcp_servers(project_root.as_deref())
                .into_iter()
                .map(|config| (config.name(), config)),
        );
    }

    deduplicate_cli_builtins(&all, &mut cli_flag_extensions);

    let cli_start = all.len();
    let renameable = cli_flag_extensions
        .iter()
        .enumerate()
        .filter_map(|(index, (_, _, renameable))| renameable.then_some(cli_start + index))
        .collect();
    all.extend(
        cli_flag_extensions
            .into_iter()
            .map(|(label, config, _)| (label, config)),
    );
    disambiguate_stdio_extension_names(&mut all, &renameable)
        .map_err(ExtensionError::ConfigError)?;

    Ok(all.into_iter().map(|(_, config)| config).collect())
}

async fn configure_session_prompts(
    session: &CliSession,
    config: &Config,
    session_config: &SessionBuilderConfig,
) {
    if let Some(ref additional_prompt) = session_config.additional_system_prompt {
        session
            .agent
            .extend_system_prompt("additional".to_string(), additional_prompt.clone())
            .await;
    }

    let system_prompt_file: Option<String> = config.get_param("GOOSE_SYSTEM_PROMPT_FILE_PATH").ok();
    if let Some(ref path) = system_prompt_file {
        let override_prompt = std::fs::read_to_string(path).unwrap_or_else(|e| {
            output::render_error(&format!(
                "Failed to read system prompt file '{}': {}",
                path, e
            ));
            process::exit(1);
        });
        session.agent.override_system_prompt(override_prompt).await;
    }
}

pub async fn build_session(session_config: SessionBuilderConfig) -> CliSession {
    #[cfg(feature = "telemetry")]
    goose::posthog::set_session_context("cli", session_config.resume);

    let config = Config::global();
    let agent: Agent = Agent::new();

    if session_config.container.is_some() {
        agent.set_container(session_config.container.clone()).await;
    }

    let session_manager = agent.config.session_manager.clone();

    let (saved_provider, saved_model_config) = if session_config.resume {
        if let Some(ref session_id) = session_config.session_id {
            match session_manager.get_session(session_id, false).await {
                Ok(session_data) => (session_data.provider_name, session_data.model_config),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let saved_provider_for_validation = saved_provider.clone();
    let saved_model_for_validation = saved_model_config
        .as_ref()
        .map(|model_config| model_config.model_name.clone());
    let resolved =
        resolve_provider_and_model(&session_config, config, saved_provider, saved_model_config)
            .await;

    let recipe = session_config.recipe.as_ref();

    agent
        .apply_recipe_components(recipe.and_then(|r| r.response.clone()), true)
        .await
        .unwrap_or_else(|error| {
            output::render_error(&format!("Invalid recipe response: {error}"));
            process::exit(1);
        });

    let session_id =
        resolve_session_id(&session_config, &session_manager, agent.config.goose_mode).await;

    if session_config.resume {
        handle_resumed_session_workdir(&agent, &session_id, session_config.interactive).await;
    }

    let extensions_for_provider =
        match collect_extension_configs(&agent, &session_config, recipe, &session_id).await {
            Ok(exts) => exts,
            Err(e) => {
                output::render_error(&format!("Failed to collect extensions: {}", e));
                process::exit(1);
            }
        };

    let (new_provider, effective_provider_name, effective_model_name, effective_model_config) =
        match create(&resolved.provider_name, extensions_for_provider.clone()).await {
            Ok(provider) => (
                provider,
                resolved.provider_name.clone(),
                resolved.model_name.clone(),
                resolved.model_config.clone(),
            ),
            Err(e)
                if session_config.resume
                    && session_config.provider.is_none()
                    && is_provider_unavailable_error(&e) =>
            {
                let fallback_provider = config.get_goose_provider().unwrap_or_else(|_| {
                    output::render_error("No provider configured. Run 'goose configure' first.");
                    process::exit(1);
                });
                let fallback_model = config.get_goose_model().unwrap_or_else(|_| {
                    output::render_error("No model configured. Run 'goose configure' first.");
                    process::exit(1);
                });
                eprintln!(
                    "{}",
                    style(format!(
                        "Warning: Could not create the session's original provider '{}' ({}). \
                    Falling back to the default provider '{}'.",
                        resolved.provider_name, e, fallback_provider
                    ))
                    .yellow()
                );
                let mut fallback_model_config =
                    model_config_from_user_config(fallback_provider.as_str(), &fallback_model)
                        .unwrap_or_else(|e| {
                            output::render_error(&format!(
                                "Failed to create model configuration: {}",
                                e
                            ));
                            process::exit(1);
                        });
                if !session_config.interactive {
                    fallback_model_config = fallback_model_config.with_cache_ttl_clamped();
                }
                match create(&fallback_provider, extensions_for_provider.clone()).await {
                    Ok(provider) => (
                        provider,
                        fallback_provider,
                        fallback_model,
                        fallback_model_config,
                    ),
                    Err(e2) => {
                        output::render_error(&format!(
                        "Error {}.\n\
                        Please check your system keychain and run 'goose configure' again.\n\
                        If your system is unable to use the keyring, please try setting secret key(s) via environment variables.\n\
                        For more info, see: https://goose-docs.ai/docs/troubleshooting/#keychainkeyring-errors",
                        e2
                    ));
                        process::exit(1);
                    }
                }
            }
            Err(e) => {
                output::render_error(&format!(
                "Error {}.\n\
                Please check your system keychain and run 'goose configure' again.\n\
                If your system is unable to use the keyring, please try setting secret key(s) via environment variables.\n\
                For more info, see: https://goose-docs.ai/docs/troubleshooting/#keychainkeyring-errors",
                e
            ));
                process::exit(1);
            }
        };
    tracing::info!("🤖 Using model: {}", effective_model_name);

    validate_provider_override_context(
        &session_config,
        saved_provider_for_validation.as_deref(),
        saved_model_for_validation.as_deref(),
        &effective_provider_name,
        &effective_model_name,
        new_provider.manages_own_context(),
    )
    .unwrap_or_else(|e| {
        output::render_error(&e.to_string());
        process::exit(1);
    });

    agent
        .update_provider(new_provider, effective_model_config, &session_id)
        .await
        .unwrap_or_else(|e| {
            output::render_error(&format!("Failed to initialize agent: {}", e));
            process::exit(1);
        });

    agent
        .update_goose_mode(agent.config.goose_mode, &session_id)
        .await
        .unwrap_or_else(|e| {
            output::render_error(&format!("Failed to set session mode: {}", e));
            process::exit(1);
        });

    if let Some(recipe) = session_config.recipe.clone() {
        if let Err(e) = session_manager
            .update(&session_id)
            .recipe(Some(recipe))
            .apply()
            .await
        {
            tracing::warn!("Failed to store recipe on session: {}", e);
        }
    }

    for warning in goose::config::get_warnings() {
        eprintln!("{}", style(format!("Warning: {}", warning)).yellow());
    }

    // Extensions are loaded after session creation because we may change
    // directory when resuming.
    let agent_ptr = Arc::new(agent);
    let loading_handle = match agent_ptr
        .persist_extension_configs(&session_id, extensions_for_provider.clone())
        .await
    {
        Ok(()) => AbortOnDropHandle::new(tokio::spawn({
            let agent = agent_ptr.clone();
            let sid = session_id.clone();
            async move { load_extensions(agent, extensions_for_provider, &sid).await }
        })),
        Err(error) => AbortOnDropHandle::new(tokio::spawn(async move {
            vec![ExtensionFailure { label: None, error }]
        })),
    };

    let edit_mode = config
        .get_param::<String>("EDIT_MODE")
        .ok()
        .and_then(|edit_mode| match edit_mode.to_lowercase().as_str() {
            "emacs" => Some(EditMode::Emacs),
            "vi" => Some(EditMode::Vi),
            _ => {
                eprintln!("Invalid EDIT_MODE specified, defaulting to Emacs");
                None
            }
        });

    let debug_mode = session_config.debug || config.get_param("GOOSE_DEBUG").unwrap_or(false);

    let session = CliSession::new(
        agent_ptr,
        session_id.clone(),
        debug_mode,
        session_config.scheduled_job_id.clone(),
        session_config.max_turns,
        edit_mode,
        recipe.and_then(|r| r.retry.clone()),
        session_config.output_format.clone(),
        session_config.stats,
        session_config.interactive,
        Some(loading_handle),
    )
    .await;

    configure_session_prompts(&session, config, &session_config).await;

    if !session_config.quiet {
        output::display_session_info(
            session_config.resume,
            &effective_provider_name,
            &effective_model_name,
            &Some(session_id),
        );
    }
    session
}

fn is_provider_unavailable_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("is not set")
        || msg.contains("not configured")
        || msg.contains("Configuration value not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use goose::config::{set_provider_entry, ProviderEntry};
    use goose::session::SessionManager;
    use tempfile::TempDir;

    fn stdio_names(extensions: &[&str]) -> Vec<String> {
        let parsed = parse_cli_flag_extensions(
            &extensions.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &[],
            &[],
        );
        let renameable = parsed
            .iter()
            .enumerate()
            .filter_map(|(index, (_, _, renameable))| renameable.then_some(index))
            .collect();
        let mut configs: Vec<_> = parsed
            .into_iter()
            .map(|(label, config, _)| (label, config))
            .collect();
        disambiguate_stdio_extension_names(&mut configs, &renameable).unwrap();
        configs
            .into_iter()
            .map(|(_, config)| config.name())
            .collect()
    }

    #[test]
    fn test_colliding_launcher_names_fall_back_to_the_command_line() {
        assert_eq!(
            stdio_names(&[
                "npx -y @modelcontextprotocol/server-memory",
                "npx -y @modelcontextprotocol/server-filesystem",
            ]),
            vec!["server-memory".to_string(), "server-filesystem".to_string()]
        );
    }

    #[test]
    fn test_explicit_names_survive_a_collision() {
        assert_eq!(
            stdio_names(&["python -m word_mcp", "memory:python -m memory_mcp"]),
            vec!["python".to_string(), "memory".to_string()]
        );
        assert_eq!(
            stdio_names(&[
                "word:python -m word_mcp",
                "python -m a_mcp",
                "python -m b_mcp",
            ]),
            vec![
                "word".to_string(),
                "python_m_a_mcp".to_string(),
                "python_m_b_mcp".to_string(),
            ]
        );
    }

    #[test]
    fn test_normalized_names_are_disambiguated() {
        assert_eq!(
            stdio_names(&["MyTool --server a", "mytool --server b"]),
            vec!["mytool_server_a".to_string(), "mytool_server_b".to_string()]
        );
    }

    #[test]
    fn test_cli_name_is_disambiguated_against_configured_extension() {
        let mut extensions = vec![
            (
                "configured".to_string(),
                CliSession::parse_stdio_extension("npx configured-server").unwrap(),
            ),
            (
                "cli".to_string(),
                CliSession::parse_stdio_extension("npx cli-server").unwrap(),
            ),
        ];
        disambiguate_stdio_extension_names(&mut extensions, &HashSet::from([1])).unwrap();
        assert_eq!(extensions[0].1.name(), "npx");
        assert_eq!(extensions[1].1.name(), "npx_cli-server");
    }

    #[test]
    fn test_fixed_counted_per_key_not_globally() {
        // `duplicate_fixed_names` must count *fixed entries sharing a key*,
        // not "is there more than one fixed entry at all" -- otherwise a
        // fixed entry with a unique name, sitting alongside a fixed+renameable
        // collision on a different key, would wrongly trip the same error.
        let mut extensions = vec![
            (
                "configured a".to_string(),
                // Fixed, name "word" -- unique, no collision with anything.
                CliSession::parse_stdio_extension("word:npx server-a").unwrap(),
            ),
            (
                "configured b".to_string(),
                // Fixed, name "npx" -- collides with the renameable entry below.
                CliSession::parse_stdio_extension("npx server-b").unwrap(),
            ),
            (
                "cli".to_string(),
                // Renameable, also defaults to "npx".
                CliSession::parse_stdio_extension("npx cli-server").unwrap(),
            ),
        ];
        disambiguate_stdio_extension_names(&mut extensions, &HashSet::from([2])).unwrap();
        assert_eq!(extensions[0].1.name(), "word");
        assert_eq!(extensions[1].1.name(), "npx");
        assert_eq!(extensions[2].1.name(), "npx_cli-server");
    }

    #[test]
    fn test_duplicate_explicit_names_are_rejected() {
        let mut extensions = vec![
            (
                "first".to_string(),
                CliSession::parse_stdio_extension("memory:npx first").unwrap(),
            ),
            (
                "second".to_string(),
                CliSession::parse_stdio_extension("Memory:npx second").unwrap(),
            ),
        ];
        let error =
            disambiguate_stdio_extension_names(&mut extensions, &HashSet::new()).unwrap_err();
        assert!(error
            .to_string()
            .contains("extension name 'memory' is already in use"));
    }

    #[test]
    fn test_cli_builtin_reuses_configured_registered_extension() {
        let configured = CliSession::parse_builtin_extensions("developer")
            .into_iter()
            .next()
            .unwrap();
        let extensions = vec![("configured".to_string(), configured)];
        let mut cli = parse_cli_flag_extensions(&[], &[], &["developer".to_string()]);

        deduplicate_cli_builtins(&extensions, &mut cli);

        assert!(cli.is_empty());
    }

    #[test]
    fn test_cli_builtin_does_not_reuse_different_extension_with_same_name() {
        let extensions = vec![(
            "configured".to_string(),
            CliSession::parse_stdio_extension("developer:npx custom-developer").unwrap(),
        )];
        let mut cli = parse_cli_flag_extensions(&[], &[], &["developer".to_string()]);

        deduplicate_cli_builtins(&extensions, &mut cli);

        assert_eq!(cli.len(), 1);
    }

    #[test]
    fn test_identical_commands_still_get_distinct_names() {
        assert_eq!(
            stdio_names(&["python -m word_mcp", "python -m word_mcp"]),
            vec![
                "python_m_word_mcp".to_string(),
                "python_m_word_mcp_2".to_string()
            ]
        );
    }

    fn test_config(temp_dir: &TempDir) -> Config {
        Config::new_with_file_secrets(
            temp_dir.path().join("config.yaml"),
            temp_dir.path().join("secrets.yaml"),
        )
        .unwrap()
    }

    fn clear_provider_env() -> env_lock::EnvGuard<'static> {
        env_lock::lock_env([
            ("GOOSE_PROVIDER", None::<&str>),
            ("GOOSE_MODEL", None::<&str>),
        ])
    }

    fn saved_model_config(model_name: &str) -> goose_providers::model::ModelConfig {
        goose_providers::model::ModelConfig::new(model_name).with_merged_request_params(
            std::collections::HashMap::from([(
                "anthropic_beta".to_string(),
                serde_json::json!(["prompt-caching-2024-07-31"]),
            )]),
        )
    }

    #[test]
    fn test_session_builder_config_creation() {
        let config = SessionBuilderConfig {
            session_id: None,
            resume: false,
            fork: false,
            no_session: false,
            extensions: vec!["echo test".to_string()],
            streamable_http_extensions: vec![StreamableHttpOptions {
                url: "http://localhost:8080/mcp".to_string(),
                timeout: goose::config::DEFAULT_EXTENSION_TIMEOUT,
            }],
            builtins: vec!["developer".to_string()],
            no_profile: false,
            recipe: None,
            additional_system_prompt: Some("Test prompt".to_string()),
            provider: None,
            model: None,
            debug: true,
            max_tool_repetitions: Some(5),
            max_turns: None,
            scheduled_job_id: None,
            interactive: true,
            quiet: false,
            output_format: "text".to_string(),
            container: None,
            stats: false,
        };

        assert_eq!(config.extensions.len(), 1);
        assert_eq!(config.streamable_http_extensions.len(), 1);
        assert_eq!(config.builtins.len(), 1);
        assert!(config.debug);
        assert_eq!(config.max_tool_repetitions, Some(5));
        assert!(config.max_turns.is_none());
        assert!(config.scheduled_job_id.is_none());
        assert!(config.interactive);
        assert!(!config.quiet);
    }

    #[test]
    fn test_session_builder_config_default() {
        let config = SessionBuilderConfig::default();

        assert!(config.session_id.is_none());
        assert!(!config.resume);
        assert!(!config.no_session);
        assert!(config.extensions.is_empty());
        assert!(config.streamable_http_extensions.is_empty());
        assert!(config.builtins.is_empty());
        assert!(!config.no_profile);
        assert!(config.recipe.is_none());
        assert!(config.additional_system_prompt.is_none());
        assert!(!config.debug);
        assert!(config.max_tool_repetitions.is_none());
        assert!(config.max_turns.is_none());
        assert!(config.scheduled_job_id.is_none());
        assert!(!config.interactive);
        assert!(!config.quiet);
        assert!(!config.fork);
    }

    #[tokio::test]
    async fn resume_provider_override_uses_target_provider_model() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        set_provider_entry(
            &config,
            "openai",
            &ProviderEntry {
                enabled: true,
                model: "gpt-5.4".to_string(),
                configured: true,
            },
        )
        .unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("openai".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(goose_providers::model::ModelConfig::new(
                "claude-sonnet-4-6",
            )),
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "gpt-5.4");
        assert_eq!(resolved.model_config.model_name, "gpt-5.4");
    }

    #[tokio::test]
    async fn matching_provider_override_preserves_configured_model() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        config.set_param("GOOSE_PROVIDER", "openai").unwrap();
        config.set_param("GOOSE_MODEL", "my-custom-model").unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                provider: Some("openai".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            None,
            None,
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "my-custom-model");
        assert_eq!(resolved.model_config.model_name, "my-custom-model");
    }

    #[tokio::test]
    async fn matching_environment_model_overrides_saved_model() {
        let _guard = env_lock::lock_env([
            ("GOOSE_PROVIDER", Some("openai")),
            ("GOOSE_MODEL", Some("environment-model")),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        set_provider_entry(
            &config,
            "openai",
            &ProviderEntry {
                enabled: true,
                model: "configured-model".to_string(),
                configured: true,
            },
        )
        .unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("openai".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("openai".to_string()),
            Some(goose_providers::model::ModelConfig::new("saved-model")),
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "environment-model");
        assert_eq!(resolved.model_config.model_name, "environment-model");
    }

    #[tokio::test]
    async fn matching_provider_override_preserves_saved_model_over_configured_model() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        config.set_param("active_provider", "openai").unwrap();
        set_provider_entry(
            &config,
            "openai",
            &ProviderEntry {
                enabled: true,
                model: "configured-model".to_string(),
                configured: true,
            },
        )
        .unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("openai".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("openai".to_string()),
            Some(goose_providers::model::ModelConfig::new("saved-model")),
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "saved-model");
        assert_eq!(resolved.model_config.model_name, "saved-model");
    }

    #[tokio::test]
    async fn matching_provider_override_preserves_recipe_model() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        config.set_param("GOOSE_PROVIDER", "openai").unwrap();
        config.set_param("GOOSE_MODEL", "configured-model").unwrap();
        let recipe = serde_json::from_value(serde_json::json!({
            "version": "1.0.0",
            "title": "test recipe",
            "description": "test recipe",
            "instructions": "test",
            "settings": {
                "goose_provider": "openai",
                "goose_model": "recipe-model"
            }
        }))
        .unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                provider: Some("openai".to_string()),
                recipe: Some(recipe),
                ..SessionBuilderConfig::default()
            },
            &config,
            None,
            None,
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "recipe-model");
        assert_eq!(resolved.model_config.model_name, "recipe-model");
    }

    #[tokio::test]
    async fn conflicting_recipe_model_is_ignored_for_provider_override() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        set_provider_entry(
            &config,
            "openai",
            &ProviderEntry {
                enabled: true,
                model: "openai-model".to_string(),
                configured: true,
            },
        )
        .unwrap();
        let recipe = serde_json::from_value(serde_json::json!({
            "version": "1.0.0",
            "title": "test recipe",
            "description": "test recipe",
            "instructions": "test",
            "settings": {
                "goose_provider": "anthropic",
                "goose_model": "claude-model"
            }
        }))
        .unwrap();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                provider: Some("openai".to_string()),
                recipe: Some(recipe),
                ..SessionBuilderConfig::default()
            },
            &config,
            None,
            None,
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, "openai-model");
    }

    #[tokio::test]
    async fn resume_provider_override_uses_target_provider_default_instead_of_active_model() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        config.set_param("GOOSE_PROVIDER", "anthropic").unwrap();
        config
            .set_param("GOOSE_MODEL", "claude-sonnet-4-6")
            .unwrap();
        let expected_model = goose::providers::get_from_registry("openai")
            .await
            .unwrap()
            .metadata()
            .default_model
            .clone();

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("openai".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(goose_providers::model::ModelConfig::new(
                "claude-sonnet-4-6",
            )),
        )
        .await;

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model_name, expected_model);
        assert_ne!(resolved.model_name, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn resume_provider_override_rebuilds_same_named_model_config() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let saved_model_config = saved_model_config("current");

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("openai".to_string()),
                model: Some("current".to_string()),
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(saved_model_config),
        )
        .await;

        assert!(!resolved
            .model_config
            .request_params
            .as_ref()
            .is_some_and(|params| params.contains_key("anthropic_beta")));
    }

    #[tokio::test]
    async fn resume_same_provider_reuses_saved_model_config() {
        let _guard = clear_provider_env();
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let saved_model_config = saved_model_config("current");

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(saved_model_config),
        )
        .await;

        assert!(resolved
            .model_config
            .request_params
            .as_ref()
            .is_some_and(|params| params.contains_key("anthropic_beta")));
    }

    #[tokio::test]
    async fn resume_rederives_cache_ttl_from_config_not_session() {
        let _guard = env_lock::lock_env([
            ("GOOSE_PROVIDER", None::<&str>),
            ("GOOSE_MODEL", None::<&str>),
            ("GOOSE_CACHE_TTL", Some("1h")),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let saved =
            goose_providers::model::ModelConfig::new("claude-sonnet-4-6").with_cache_ttl("5m");

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                interactive: true,
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(saved),
        )
        .await;

        assert_eq!(resolved.model_config.cache_ttl().as_deref(), Some("1h"));
    }

    #[tokio::test]
    async fn resume_drops_saved_cache_ttl_when_config_absent() {
        let _guard = env_lock::lock_env([
            ("GOOSE_PROVIDER", None::<&str>),
            ("GOOSE_MODEL", None::<&str>),
            ("GOOSE_CACHE_TTL", None::<&str>),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let saved =
            goose_providers::model::ModelConfig::new("claude-sonnet-4-6").with_cache_ttl("1h");

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                interactive: true,
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(saved),
        )
        .await;

        assert!(resolved.model_config.cache_ttl().is_none());
    }

    #[tokio::test]
    async fn headless_resume_clamps_rederived_cache_ttl() {
        let _guard = env_lock::lock_env([
            ("GOOSE_PROVIDER", None::<&str>),
            ("GOOSE_MODEL", None::<&str>),
            ("GOOSE_CACHE_TTL", Some("1h")),
        ]);
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let saved = goose_providers::model::ModelConfig::new("claude-sonnet-4-6");

        let resolved = resolve_provider_and_model(
            &SessionBuilderConfig {
                resume: true,
                interactive: false,
                ..SessionBuilderConfig::default()
            },
            &config,
            Some("anthropic".to_string()),
            Some(saved),
        )
        .await;

        assert_eq!(resolved.model_config.cache_ttl().as_deref(), Some("5m"));
    }

    #[test]
    fn resumed_provider_override_rejects_context_owning_provider() {
        let error = validate_provider_override_context(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("claude-code".to_string()),
                ..SessionBuilderConfig::default()
            },
            Some("openai"),
            Some("gpt-5.4"),
            "claude-code",
            "claude-sonnet-4-6",
            true,
        )
        .expect_err("context-owning replacement provider should be rejected");

        assert_eq!(
            error.to_string(),
            "Cannot resume with provider or model changes because provider 'claude-code' manages its own conversation context. Start a new session to use this provider or model."
        );
    }

    #[test]
    fn resumed_same_context_owning_provider_override_is_allowed() {
        let result = validate_provider_override_context(
            &SessionBuilderConfig {
                resume: true,
                provider: Some("claude-code".to_string()),
                ..SessionBuilderConfig::default()
            },
            Some("claude-code"),
            Some("current"),
            "claude-code",
            "current",
            true,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn resumed_context_owning_provider_rejects_model_change() {
        let result = validate_provider_override_context(
            &SessionBuilderConfig {
                resume: true,
                model: Some("new-model".to_string()),
                ..SessionBuilderConfig::default()
            },
            Some("claude-code"),
            Some("current"),
            "claude-code",
            "new-model",
            true,
        );

        assert!(result.is_err());
    }

    #[test]
    fn new_session_provider_override_allows_context_owning_provider() {
        let result = validate_provider_override_context(
            &SessionBuilderConfig {
                provider: Some("claude-code".to_string()),
                ..SessionBuilderConfig::default()
            },
            None,
            None,
            "claude-code",
            "current",
            true,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn resumed_session_without_provider_override_allows_context_owning_provider() {
        let result = validate_provider_override_context(
            &SessionBuilderConfig {
                resume: true,
                ..SessionBuilderConfig::default()
            },
            Some("claude-code"),
            Some("current"),
            "claude-code",
            "current",
            true,
        );

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_implicit_resume_ignores_newer_scheduled_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = SessionManager::new(temp_dir.path().to_path_buf());
        let goose_mode = GooseMode::default();

        let user_session = session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "User session".to_string(),
                SessionType::User,
                goose_mode,
            )
            .await
            .unwrap();
        session_manager
            .create_session(
                temp_dir.path().to_path_buf(),
                "Scheduled job: test".to_string(),
                SessionType::Scheduled,
                goose_mode,
            )
            .await
            .unwrap();

        let resolved = resolve_session_id(
            &SessionBuilderConfig {
                resume: true,
                ..SessionBuilderConfig::default()
            },
            &session_manager,
            goose_mode,
        )
        .await;

        assert_eq!(resolved, user_session.id);
    }

    #[test]
    fn test_truncate_with_ellipsis() {
        assert_eq!(truncate_with_ellipsis("abc", 5), "abc");

        assert_eq!(truncate_with_ellipsis("abcde", 5), "abcde");

        assert_eq!(truncate_with_ellipsis("abcdef", 5), "abcde…");
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hello…");

        assert_eq!(truncate_with_ellipsis("", 5), "");
    }
}
