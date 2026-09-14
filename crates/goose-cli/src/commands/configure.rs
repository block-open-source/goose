use crate::recipes::github_recipe::GOOSE_RECIPE_GITHUB_REPO_CONFIG_KEY;
use cliclack::spinner;
use console::style;
use goose::agents::extension::{ToolInfo, PLATFORM_EXTENSIONS};
use goose::agents::extension_manager::get_parameter_names;
use goose::agents::Agent;
use goose::agents::{extension::Envs, ExtensionConfig};
use goose::config::declarative_providers::{
    create_custom_provider, remove_custom_provider, AuthConfig, CreateCustomProviderParams,
};
use goose::config::extensions::{
    get_all_extension_names, get_all_extensions, get_enabled_extensions, get_extension_by_name,
    name_to_key, remove_extension, set_extension, set_extension_enabled,
};
use goose::config::paths::Paths;
use goose::config::permission::PermissionLevel;
use goose::config::signup_tetrate::TetrateAuth;
use goose::config::{
    configure_tetrate, Config, ConfigError, ExperimentManager, ExtensionEntry, GooseMode,
    PermissionManager,
};
#[cfg(feature = "telemetry")]
use goose::posthog::{get_telemetry_choice, TELEMETRY_ENABLED_KEY};
use goose::providers::base::ConfigKey;
use goose::providers::provider_test::test_provider_configuration;
use goose::providers::{create, providers, retry_operation, RetryConfig};
use goose::session::SessionType;
use goose_providers::thinking::ThinkingEffort;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};

// useful for light themes where there is no discernible colour contrast between
// cursor-selected and cursor-unselected items.
const MULTISELECT_VISIBILITY_HINT: &str = "<";
const MAX_PROVIDER_ROWS: usize = 10;
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

type ProviderItem = (String, String, String);

#[derive(Clone, PartialEq, Eq)]
enum ProviderChoice {
    Provider(String),
    Search,
    SearchAgain,
}

fn provider_choice_items(items: &[ProviderItem]) -> Vec<(ProviderChoice, String, String)> {
    items
        .iter()
        .map(|(name, label, hint)| {
            (
                ProviderChoice::Provider(name.clone()),
                label.clone(),
                hint.clone(),
            )
        })
        .collect()
}

fn move_selected_item_into_view<T>(
    items: &mut Vec<T>,
    selected_index: Option<usize>,
    visible_rows: usize,
) {
    if let Some(index) = selected_index.filter(|&index| index >= visible_rows) {
        let selected = items.remove(index);
        items.insert(0, selected);
    }
}

fn fuzzy_filter_provider_items(items: &[ProviderItem], query: &str) -> Vec<ProviderItem> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return items.to_vec();
    }

    let query_words: Vec<_> = query.split_whitespace().collect();
    let mut scored_items: Vec<_> = items
        .iter()
        .filter_map(|item| {
            let label = item.1.to_lowercase();
            let similarity = strsim::jaro_winkler(&label, &query);
            let word_match_bonus =
                query_words.iter().all(|word| label.contains(*word)) as u8 as f64;
            let score = similarity + word_match_bonus;
            (score > 0.6).then_some((score, item))
        })
        .collect();

    scored_items.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored_items
        .into_iter()
        .map(|(_, item)| item.clone())
        .collect()
}

fn search_provider_dialog(provider_items: &[ProviderItem]) -> anyhow::Result<String> {
    let mut query = String::new();

    loop {
        let input: String = cliclack::input("Search model providers")
            .placeholder("e.g., OpenAI, Anthropic, local")
            .default_input(&query)
            .interact()?;
        query = input.trim().to_string();

        let filtered_items = fuzzy_filter_provider_items(provider_items, &query);
        if filtered_items.is_empty() {
            cliclack::log::warning("No matching providers. Try a different search term.")?;
            continue;
        }

        let mut items = provider_choice_items(&filtered_items);
        items.push((
            ProviderChoice::SearchAgain,
            "Search again...".to_string(),
            "Enter a different search term".to_string(),
        ));

        match cliclack::select("Which model provider should we use?")
            .items(&items)
            .max_rows(MAX_PROVIDER_ROWS)
            .interact()?
        {
            ProviderChoice::SearchAgain => continue,
            ProviderChoice::Provider(name) => return Ok(name),
            ProviderChoice::Search => {
                unreachable!("Search entry is not added to the results list")
            }
        }
    }
}

struct CursorRestoreGuard;

impl Drop for CursorRestoreGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(SHOW_CURSOR);
        let _ = stdout.flush();
    }
}

pub async fn handle_configure() -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "goose configure requires an interactive terminal.\n\
             If you installed via 'curl ... | bash', run 'goose configure' separately after installation."
        );
    }

    let _cursor_restore = CursorRestoreGuard;
    let config = Config::global();

    if !config.exists() {
        handle_first_time_setup(config).await
    } else {
        handle_existing_config().await
    }
}

#[cfg(feature = "telemetry")]
pub fn configure_telemetry_consent_dialog() -> anyhow::Result<bool> {
    let config = Config::global();

    println!();
    println!("{}", style("Help improve goose").bold());
    println!();
    println!(
        "{}",
        style("Would you like to help improve goose by sharing anonymous usage data?").dim()
    );
    println!(
        "{}",
        style("This helps us understand how goose is used and identify areas for improvement.")
            .dim()
    );
    println!();
    println!("{}", style("What we collect:").dim());
    println!(
        "{}",
        style("  • Operating system, version, and architecture").dim()
    );
    println!("{}", style("  • goose version and install method").dim());
    println!("{}", style("  • Provider and model used").dim());
    println!(
        "{}",
        style("  • Extensions and tool usage counts (names only)").dim()
    );
    println!(
        "{}",
        style("  • Session metrics (duration, interaction count, token usage)").dim()
    );
    println!(
        "{}",
        style("  • Error types (e.g., \"rate_limit\", \"auth\" - no details)").dim()
    );
    println!();
    println!(
        "{}",
        style("We never collect your conversations, code, tool arguments, error messages,").dim()
    );
    println!(
        "{}",
        style("or any personal data. You can change this anytime with 'goose configure'.").dim()
    );
    println!();

    let enabled = cliclack::confirm("Share anonymous usage data to help improve goose?")
        .initial_value(true)
        .interact()?;

    config.set_param(TELEMETRY_ENABLED_KEY, enabled)?;

    if enabled {
        let _ = cliclack::log::success("Thank you for helping improve goose!");
    } else {
        let _ = cliclack::log::info("Telemetry disabled. You can enable it anytime in settings.");
    }

    Ok(enabled)
}

async fn handle_first_time_setup(config: &Config) -> anyhow::Result<()> {
    println!();
    println!("{}", style("Welcome to goose! Let's get you set up.").dim());
    println!(
        "{}",
        style("  you can rerun this command later to update your configuration").dim()
    );
    println!();

    #[cfg(feature = "telemetry")]
    configure_telemetry_consent_dialog()?;

    println!();
    cliclack::intro(style(" goose-configure ").on_cyan().black())?;

    let setup_method = cliclack::select("How would you like to set up your provider?")
        .item(
            "openrouter",
            "OpenRouter Login (Recommended)",
            "Sign in with OpenRouter to automatically configure models",
        )
        .item(
            "tetrate",
            "Tetrate Agent Router Service Login",
            "Sign in with Tetrate Agent Router Service to automatically configure models",
        )
        .item(
            "manual",
            "Manual Configuration",
            "Choose a provider and enter credentials manually",
        )
        .interact()?;

    match setup_method {
        "openrouter" => {
            if let Err(e) = handle_openrouter_auth().await {
                let _ = config.clear();
                println!(
                    "\n  {} OpenRouter authentication failed: {} \n  Please try again or use manual configuration",
                    style("Error").red().italic(),
                    e,
                );
            }
        }
        "tetrate" => {
            if let Err(e) = handle_tetrate_auth().await {
                let _ = config.clear();
                println!(
                    "\n  {} Tetrate Agent Router Service authentication failed: {} \n  Please try again or use manual configuration",
                    style("Error").red().italic(),
                    e,
                );
            }
        }
        "manual" => handle_manual_provider_setup(config).await,
        _ => unreachable!(),
    }
    Ok(())
}

async fn handle_manual_provider_setup(config: &Config) {
    match configure_provider_dialog().await {
        Ok(true) => {
            println!(
                "\n  {}: Run '{}' again to adjust your config or add extensions",
                style("Tip").green().italic(),
                style("goose configure").cyan()
            );
            set_extension(ExtensionEntry {
                enabled: true,
                config: ExtensionConfig::default(),
            });
        }
        Ok(false) => {
            let _ = config.clear();
            println!(
                "\n  {}: We did not save your config, inspect your credentials\n   and run '{}' again to ensure goose can connect",
                style("Warning").yellow().italic(),
                style("goose configure").cyan()
            );
        }
        Err(e) => {
            let _ = config.clear();
            print_manual_config_error(&e);
        }
    }
}

fn print_manual_config_error(e: &anyhow::Error) {
    match e.downcast_ref::<ConfigError>() {
        Some(ConfigError::NotFound(key)) => {
            println!(
                "\n  {} Required configuration key '{}' not found \n  Please provide this value and run '{}' again",
                style("Error").red().italic(),
                key,
                style("goose configure").cyan()
            );
        }
        Some(ConfigError::KeyringError(msg)) => {
            print_keyring_error(msg);
        }
        Some(ConfigError::DeserializeError(msg)) => {
            println!(
                "\n  {} Invalid configuration value: {} \n  Please check your input and run '{}' again",
                style("Error").red().italic(),
                msg,
                style("goose configure").cyan()
            );
        }
        Some(ConfigError::FileError(err)) => {
            println!(
                "\n  {} Failed to access config file: {} \n  Please check file permissions and run '{}' again",
                style("Error").red().italic(),
                err,
                style("goose configure").cyan()
            );
        }
        Some(ConfigError::DirectoryError(msg)) => {
            println!(
                "\n  {} Failed to access config directory: {} \n  Please check directory permissions and run '{}' again",
                style("Error").red().italic(),
                msg,
                style("goose configure").cyan()
            );
        }
        _ => {
            println!(
                "\n  {} {} \n  We did not save your config, inspect your credentials\n   and run '{}' again to ensure goose can connect",
                style("Error").red().italic(),
                e,
                style("goose configure").cyan()
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn print_keyring_error(msg: &str) {
    println!(
        "\n  {} Failed to access secure storage (keyring): {} \n  Please check your system keychain and run '{}' again. \n  If your system is unable to use the keyring, please try setting secret key(s) via environment variables.",
        style("Error").red().italic(),
        msg,
        style("goose configure").cyan()
    );
}

#[cfg(target_os = "windows")]
fn print_keyring_error(msg: &str) {
    println!(
        "\n  {} Failed to access Windows Credential Manager: {} \n  Please check Windows Credential Manager and run '{}' again. \n  If your system is unable to use the Credential Manager, please try setting secret key(s) via environment variables.",
        style("Error").red().italic(),
        msg,
        style("goose configure").cyan()
    );
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn print_keyring_error(msg: &str) {
    println!(
        "\n  {} Failed to access secure storage: {} \n  Please check your system's secure storage and run '{}' again. \n  If your system is unable to use secure storage, please try setting secret key(s) via environment variables.",
        style("Error").red().italic(),
        msg,
        style("goose configure").cyan()
    );
}

async fn handle_existing_config() -> anyhow::Result<()> {
    let config_dir = Paths::config_dir().display().to_string();

    println!();
    println!(
        "{}",
        style("This will update your existing config files").dim()
    );
    println!(
        "{} {}",
        style("  if you prefer, you can edit them directly at").dim(),
        config_dir
    );
    println!();

    cliclack::intro(style(" goose-configure ").on_cyan().black())?;
    let action = cliclack::select("What would you like to configure?")
        .item(
            "providers",
            "Configure Providers",
            "Change provider or update credentials",
        )
        .item(
            "custom_providers",
            "Custom Providers",
            "Add custom provider with compatible API",
        )
        .item("add", "Add Extension", "Connect to a new extension")
        .item(
            "toggle",
            "Toggle Extensions",
            "Enable or disable connected extensions",
        )
        .item("remove", "Remove Extension", "Remove an extension")
        .item(
            "settings",
            "goose settings",
            "Set the goose mode, Tool Output, Tool Permissions, Experiment, goose recipe github repo and more",
        )
        .interact()?;

    match action {
        "toggle" => toggle_extensions_dialog(),
        "add" => configure_extensions_dialog(),
        "remove" => remove_extension_dialog(),
        "settings" => configure_settings_dialog().await,
        "providers" => configure_provider_dialog().await.map(|_| ()),
        "custom_providers" => configure_custom_provider_dialog().await,
        _ => unreachable!(),
    }
}

/// Helper function to handle OAuth configuration for a provider
async fn handle_oauth_configuration(provider_name: &str, key_name: &str) -> anyhow::Result<()> {
    let _ = cliclack::log::info(format!(
        "Configuring {} using OAuth device code flow...",
        key_name
    ));

    // Create a temporary provider instance to handle OAuth
    match create(provider_name, Vec::new()).await {
        Ok(provider) => match provider.configure_oauth().await {
            Ok(_) => {
                let _ = cliclack::log::success("OAuth authentication completed successfully!");
                Ok(())
            }
            Err(e) => {
                let _ = cliclack::log::error(format!("Failed to authenticate: {}", e));
                Err(anyhow::anyhow!(
                    "OAuth authentication failed for {}: {}",
                    key_name,
                    e
                ))
            }
        },
        Err(e) => {
            let _ = cliclack::log::error(format!("Failed to create provider for OAuth: {}", e));
            Err(anyhow::anyhow!(
                "Failed to create provider for OAuth: {}",
                e
            ))
        }
    }
}

const UNLISTED_MODEL_KEY: &str = "__unlisted__";

fn interactive_model_search(
    models: &[String],
    provider_meta: &goose::providers::base::ProviderMetadata,
) -> anyhow::Result<String> {
    const MAX_VISIBLE: usize = 30;
    let mut query = String::new();

    loop {
        let _ = cliclack::clear_screen();

        let _ = cliclack::log::info(format!(
            "🔍 {} models available. Type to filter.",
            models.len()
        ));

        let input: String = cliclack::input("Filtering models, press Enter to search")
            .placeholder("e.g., gpt, sonnet, llama, qwen")
            .default_input(&query)
            .interact::<String>()?;
        query = input.trim().to_string();

        let filtered: Vec<String> = if query.is_empty() {
            models.to_vec()
        } else {
            let q = query.to_lowercase();
            models
                .iter()
                .filter(|m| m.to_lowercase().contains(&q))
                .cloned()
                .collect()
        };

        if filtered.is_empty() {
            let selection = cliclack::select("No matching models. What would you like to do?")
                .item(
                    "__new_search__",
                    "Start a new search...",
                    "Enter a different search term",
                )
                .item(UNLISTED_MODEL_KEY, "Enter a model not listed...", "")
                .interact()?;

            if selection == UNLISTED_MODEL_KEY {
                return prompt_unlisted_model(provider_meta);
            }

            query.clear();
            continue;
        }

        let mut items: Vec<(String, String, &str)> = filtered
            .iter()
            .take(MAX_VISIBLE)
            .map(|m| (m.clone(), m.clone(), ""))
            .collect();

        if filtered.len() > MAX_VISIBLE {
            items.insert(
                0,
                (
                    "__refine__".to_string(),
                    format!(
                        "Refine search to see more (showing {} of {} results)",
                        MAX_VISIBLE,
                        filtered.len()
                    ),
                    "Too many matches",
                ),
            );
        } else {
            items.insert(
                0,
                (
                    "__new_search__".to_string(),
                    "Start a new search...".to_string(),
                    "Enter a different search term",
                ),
            );
        }

        items.push((
            UNLISTED_MODEL_KEY.to_string(),
            "Enter a model not listed...".to_string(),
            "",
        ));

        let selection = cliclack::select("Select a model:")
            .items(&items)
            .interact()?;

        if selection == "__refine__" {
            continue;
        } else if selection == "__new_search__" {
            query.clear();
            continue;
        } else if selection == UNLISTED_MODEL_KEY {
            return prompt_unlisted_model(provider_meta);
        } else {
            return Ok(selection);
        }
    }
}

fn select_model_from_list(
    models: &[String],
    provider_meta: &goose::providers::base::ProviderMetadata,
) -> anyhow::Result<String> {
    const MAX_MODELS: usize = 10;

    // Smart model selection:
    // If we have more than MAX_MODELS models, show the recommended models with additional search option.
    // Otherwise, show all models without search.
    if models.len() > MAX_MODELS {
        let recommended_models: Vec<String> = provider_meta
            .known_models
            .iter()
            .map(|m| m.name.clone())
            .filter(|name| models.contains(name))
            .collect();

        if !recommended_models.is_empty() {
            let mut model_items: Vec<(String, String, &str)> = recommended_models
                .iter()
                .map(|m| (m.clone(), m.clone(), "Recommended"))
                .collect();

            model_items.insert(
                0,
                (
                    "search_all".to_string(),
                    "Search all models...".to_string(),
                    "Search complete model list",
                ),
            );

            model_items.push((
                UNLISTED_MODEL_KEY.to_string(),
                "Enter a model not listed...".to_string(),
                "",
            ));

            let selection = cliclack::select("Select a model:")
                .items(&model_items)
                .interact()?;

            if selection == "search_all" {
                interactive_model_search(models, provider_meta)
            } else if selection == UNLISTED_MODEL_KEY {
                prompt_unlisted_model(provider_meta)
            } else {
                Ok(selection)
            }
        } else {
            interactive_model_search(models, provider_meta)
        }
    } else {
        let mut model_items: Vec<(String, String, &str)> =
            models.iter().map(|m| (m.clone(), m.clone(), "")).collect();

        model_items.push((
            UNLISTED_MODEL_KEY.to_string(),
            "Enter a model not listed...".to_string(),
            "",
        ));

        let selection = cliclack::select("Select a model:")
            .items(&model_items)
            .interact()?;

        if selection == UNLISTED_MODEL_KEY {
            prompt_unlisted_model(provider_meta)
        } else {
            Ok(selection)
        }
    }
}

fn prompt_unlisted_model(
    provider_meta: &goose::providers::base::ProviderMetadata,
) -> anyhow::Result<String> {
    let model: String = cliclack::input("Enter the model name:")
        .placeholder(&provider_meta.default_model)
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Please enter a model name")
            } else {
                Ok(())
            }
        })
        .interact()?;
    Ok(model.trim().to_string())
}

fn try_store_secret(config: &Config, key_name: &str, value: String) -> anyhow::Result<bool> {
    match config.set_secret(key_name, &value) {
        Ok(_) => Ok(true),
        Err(ConfigError::FallbackToFileStorage) => Ok(true),
        Err(e) => {
            cliclack::outro(style(format!(
                "Failed to store {} securely: {}. Please ensure your system's secure storage is accessible. Alternatively you can run with GOOSE_DISABLE_KEYRING=true or set the key in your environment variables",
                key_name, e
            )).on_red().white())?;
            Ok(false)
        }
    }
}

async fn configure_single_key(
    config: &Config,
    provider_name: &str,
    display_name: &str,
    key: &ConfigKey,
) -> anyhow::Result<bool> {
    let from_env = std::env::var(&key.name).ok();

    match from_env {
        Some(env_value) => {
            let _ = cliclack::log::info(format!("{} is set via environment variable", key.name));
            if cliclack::confirm("Would you like to save this value to your keyring?")
                .initial_value(true)
                .interact()?
            {
                if key.secret {
                    if !try_store_secret(config, &key.name, env_value)? {
                        return Ok(false);
                    }
                } else {
                    config.set_param(&key.name, &env_value)?;
                }
                let _ = cliclack::log::info(format!("Saved {} to {}", key.name, config.path()));
            }
        }
        None => {
            let existing: Result<String, _> = if key.secret {
                config.get_secret(&key.name)
            } else {
                config.get_param(&key.name)
            };

            match existing {
                Ok(_) => {
                    let _ = cliclack::log::info(format!("{} is already configured", key.name));
                    if cliclack::confirm("Would you like to update this value?").interact()? {
                        if key.oauth_flow {
                            handle_oauth_configuration(provider_name, &key.name).await?;
                        } else {
                            let value: String = if key.secret {
                                cliclack::password(format!("Enter new value for {}", key.name))
                                    .mask('▪')
                                    .interact()?
                            } else {
                                let mut input =
                                    cliclack::input(format!("Enter new value for {}", key.name));
                                if key.default.is_some() {
                                    input = input.default_input(&key.default.clone().unwrap());
                                }
                                input.interact()?
                            };

                            if key.secret {
                                if !try_store_secret(config, &key.name, value)? {
                                    return Ok(false);
                                }
                            } else {
                                config.set_param(&key.name, &value)?;
                            }
                        }
                    }
                }
                Err(_) => {
                    if key.oauth_flow {
                        handle_oauth_configuration(provider_name, &key.name).await?;
                    } else if !key.required && key.secret {
                        if cliclack::confirm(format!(
                            "Would you like to set {}? (optional)",
                            key.name
                        ))
                        .initial_value(true)
                        .interact()?
                        {
                            let value: String =
                                cliclack::password(format!("Enter value for {}", key.name))
                                    .mask('▪')
                                    .interact()?;
                            if !try_store_secret(config, &key.name, value)? {
                                return Ok(false);
                            }
                        }
                    } else {
                        let prompt = if key.required {
                            format!(
                                "Provider {} requires {}, please enter a value",
                                display_name, key.name
                            )
                        } else {
                            format!("Enter {} (optional, press Enter to skip)", key.name)
                        };

                        let value: String = if key.secret {
                            cliclack::password(&prompt).mask('▪').interact()?
                        } else {
                            let mut input = cliclack::input(&prompt);
                            if key.default.is_some() {
                                input = input.default_input(&key.default.clone().unwrap());
                            }
                            if !key.required {
                                input = input.required(false);
                            }
                            input.interact()?
                        };

                        if value.is_empty() {
                            return Ok(true);
                        }

                        if key.secret {
                            if !try_store_secret(config, &key.name, value)? {
                                return Ok(false);
                            }
                        } else {
                            config.set_param(&key.name, &value)?;
                        }
                    }
                }
            }
        }
    }
    Ok(true)
}

pub async fn configure_provider_dialog() -> anyhow::Result<bool> {
    // Get global config instance
    let config = Config::global();

    let current_provider: Option<String> = config.get_goose_provider().ok();
    let mut available_providers = providers().await;
    available_providers.retain(|(provider, _)| {
        provider.deprecated.is_none() || current_provider.as_deref() == Some(&provider.name)
    });

    // Sort providers alphabetically by display name
    available_providers.sort_by(|a, b| a.0.display_name.cmp(&b.0.display_name));

    // Get current default provider if it exists
    let current_provider_index = current_provider.as_ref().and_then(|current_provider| {
        available_providers
            .iter()
            .position(|(provider, _)| &provider.name == current_provider)
    });
    let visible_provider_rows = if available_providers.len() > MAX_PROVIDER_ROWS {
        MAX_PROVIDER_ROWS - 1
    } else {
        MAX_PROVIDER_ROWS
    };
    move_selected_item_into_view(
        &mut available_providers,
        current_provider_index,
        visible_provider_rows,
    );

    // Create selection items from provider metadata
    let provider_items: Vec<ProviderItem> = available_providers
        .iter()
        .map(|(p, _)| {
            let description = match p
                .deprecated
                .as_ref()
                .and_then(|deprecated| deprecated.replacement.as_deref())
            {
                Some(replacement) => {
                    format!("{} Deprecated; use {replacement} instead.", p.description)
                }
                None => p.description.clone(),
            };
            (p.name.clone(), p.display_name.clone(), description)
        })
        .collect();

    let default_provider = current_provider
        .filter(|current_provider| {
            available_providers
                .iter()
                .any(|(provider, _)| &provider.name == current_provider)
        })
        .unwrap_or_default();

    // cliclack 0.5.5 does not reset its private list offset when filtering a
    // paginated select, so use a separate fuzzy-search step for long lists.
    let provider_name = if provider_items.len() > MAX_PROVIDER_ROWS {
        let mut paginated_items = provider_choice_items(&provider_items);
        paginated_items.insert(
            MAX_PROVIDER_ROWS - 1,
            (
                ProviderChoice::Search,
                "Search all providers...".to_string(),
                "Filter the complete provider list".to_string(),
            ),
        );

        match cliclack::select("Which model provider should we use?")
            .initial_value(ProviderChoice::Provider(default_provider.clone()))
            .items(&paginated_items)
            .max_rows(MAX_PROVIDER_ROWS)
            .interact()?
        {
            ProviderChoice::Search => search_provider_dialog(&provider_items)?,
            ProviderChoice::Provider(name) => name,
            ProviderChoice::SearchAgain => {
                unreachable!("SearchAgain entry is not added to the paginated list")
            }
        }
    } else {
        cliclack::select("Which model provider should we use?")
            .initial_value(default_provider.clone())
            .items(&provider_items)
            .filter_mode()
            .interact()?
    };

    // Get the selected provider's metadata
    let (provider_meta, _) = available_providers
        .iter()
        .find(|(p, _)| p.name == provider_name.as_str())
        .expect("Selected provider must exist in metadata");

    for key in provider_meta
        .config_keys
        .iter()
        .filter(|k| k.primary || k.oauth_flow)
    {
        if !configure_single_key(config, &provider_name, &provider_meta.display_name, key).await? {
            return Ok(false);
        }
    }

    let non_primary_keys: Vec<_> = provider_meta
        .config_keys
        .iter()
        .filter(|k| !k.primary && !k.oauth_flow)
        .collect();
    if !non_primary_keys.is_empty()
        && cliclack::confirm("Would you like to configure advanced settings?")
            .initial_value(false)
            .interact()?
    {
        for key in non_primary_keys {
            if !configure_single_key(config, &provider_name, &provider_meta.display_name, key)
                .await?
            {
                return Ok(false);
            }
        }
    }

    let spin = spinner();
    spin.start("Attempting to fetch supported models...");
    let temp_provider = create(&provider_name, Vec::new()).await?;
    let models_res = retry_operation(&RetryConfig::default(), || async {
        temp_provider
            .fetch_recommended_models(goose::model_config::global_toolshim())
            .await
    })
    .await;
    spin.stop(style("Model fetch complete").green());

    // Select a model: on fetch error show styled error and abort; if models available, show list; otherwise free-text input
    let model: String = match models_res {
        Err(e) => {
            // Provider hook error
            cliclack::outro(style(e.to_string()).on_red().white())?;
            return Ok(false);
        }
        Ok(models) if !models.is_empty() => select_model_from_list(&models, provider_meta)?,
        Ok(_) => {
            let default_model =
                std::env::var("GOOSE_MODEL").unwrap_or(provider_meta.default_model.clone());
            cliclack::input("Enter a model from that provider:")
                .default_input(&default_model)
                .interact()?
        }
    };

    {
        let supports_thinking = match temp_provider.fetch_model_info(&model).await {
            Ok(model_info) => model_info.reasoning,
            Err(_) => goose_providers::model::ModelConfig::new(&model).is_reasoning_model(),
        };

        if supports_thinking {
            let effort: ThinkingEffort = cliclack::select("Select thinking effort:")
                .item("off", "Off - No extended thinking", "")
                .item("low", "Low - Better latency, lighter reasoning", "")
                .item("medium", "Medium - Moderate thinking", "")
                .item("high", "High - Deep reasoning", "")
                .item("max", "Max - No constraints on thinking depth", "")
                .initial_value("off")
                .interact()?
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid thinking effort"))?;
            config.set_goose_thinking_effort(effort)?;
        }
    }

    // Test the configuration
    let spin = spinner();
    spin.start("Checking your configuration...");

    let toolshim_enabled = std::env::var("GOOSE_TOOLSHIM")
        .map(|val| val == "1" || val.to_lowercase() == "true")
        .unwrap_or(false);
    let toolshim_model = std::env::var("GOOSE_TOOLSHIM_OLLAMA_MODEL").ok();

    match test_provider_configuration(&provider_name, &model, toolshim_enabled, toolshim_model)
        .await
    {
        Ok(()) => {
            goose::config::set_active_provider(config, &provider_name, &model)?;
            print_config_file_saved()?;
            Ok(true)
        }
        Err(e) => {
            spin.stop(style(e.to_string()).red());
            cliclack::outro(
                style(format!("Failed to configure provider: {e}"))
                    .on_red()
                    .white(),
            )?;
            Ok(false)
        }
    }
}

/// Configure extensions that can be used with goose
/// Dialog for toggling which extensions are enabled/disabled
pub fn toggle_extensions_dialog() -> anyhow::Result<()> {
    for warning in goose::config::get_warnings() {
        eprintln!("{}", style(format!("Warning: {}", warning)).yellow());
    }

    let extensions = get_all_extensions();

    if extensions.is_empty() {
        cliclack::outro(
            "No extensions configured yet. Run configure and add some extensions first.",
        )?;
        return Ok(());
    }

    // Create a list of extension names and their enabled status
    let mut extension_status: Vec<(String, bool)> = extensions
        .iter()
        .map(|entry| (entry.config.name().to_string(), entry.enabled))
        .collect();

    // Sort extensions alphabetically by name
    extension_status.sort_by(|a, b| a.0.cmp(&b.0));

    // Get currently enabled extensions for the selection
    let enabled_extensions: Vec<&String> = extension_status
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(name, _)| name)
        .collect();

    // Let user toggle extensions
    let selected = cliclack::multiselect(
        "enable extensions: (use \"space\" to toggle and \"enter\" to submit)",
    )
    .required(false)
    .items(
        &extension_status
            .iter()
            .map(|(name, _)| (name, name.as_str(), MULTISELECT_VISIBILITY_HINT))
            .collect::<Vec<_>>(),
    )
    .initial_values(enabled_extensions)
    .filter_mode()
    .interact()?;

    // Update enabled status for each extension
    for name in extension_status.iter().map(|(name, _)| name) {
        set_extension_enabled(
            &name_to_key(name),
            selected.iter().any(|s| s.as_str() == name),
        );
    }

    let config = Config::global();
    cliclack::outro(format!(
        "Extension settings saved successfully to {}",
        config.path()
    ))?;
    Ok(())
}

fn prompt_extension_timeout() -> anyhow::Result<u64> {
    Ok(
        cliclack::input("Please set the timeout for this tool (in secs):")
            .placeholder(&goose::config::DEFAULT_EXTENSION_TIMEOUT.to_string())
            .validate(|input: &String| match input.parse::<u64>() {
                Ok(_) => Ok(()),
                Err(_) => Err("Please enter a valid timeout"),
            })
            .interact()?,
    )
}

fn prompt_extension_description() -> anyhow::Result<String> {
    Ok(cliclack::input("Enter a description for this extension:")
        .placeholder("Description")
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Please enter a valid description")
            } else {
                Ok(())
            }
        })
        .interact()?)
}

fn prompt_extension_name(placeholder: &str) -> anyhow::Result<String> {
    let extensions = get_all_extension_names();
    Ok(
        cliclack::input("What would you like to call this extension?")
            .placeholder(placeholder)
            .validate(move |input: &String| {
                if input.is_empty() {
                    Err("Please enter a name")
                } else if extensions.contains(input) {
                    Err("An extension with this name already exists")
                } else {
                    Ok(())
                }
            })
            .interact()?,
    )
}

fn collect_env_vars() -> anyhow::Result<(HashMap<String, String>, Vec<String>)> {
    let envs = HashMap::new();
    let mut env_keys = Vec::new();
    let config = Config::global();

    if !cliclack::confirm("Would you like to add environment variables?").interact()? {
        return Ok((envs, env_keys));
    }

    loop {
        let key: String = cliclack::input("Environment variable name:")
            .placeholder("API_KEY")
            .interact()?;

        let value: String = cliclack::password("Environment variable value:")
            .mask('▪')
            .interact()?;

        if !try_store_secret(config, &key, value)? {
            return Err(anyhow::anyhow!("Failed to store secret"));
        }
        env_keys.push(key);

        if !cliclack::confirm("Add another environment variable?").interact()? {
            break;
        }
    }

    Ok((envs, env_keys))
}

fn collect_headers() -> anyhow::Result<HashMap<String, String>> {
    let mut headers = HashMap::new();

    if !cliclack::confirm("Would you like to add custom headers?").interact()? {
        return Ok(headers);
    }

    loop {
        let key: String = cliclack::input("Header name:")
            .placeholder("Authorization")
            .interact()?;

        let value: String = cliclack::input("Header value:")
            .placeholder("Bearer token123")
            .interact()?;

        headers.insert(key, value);

        if !cliclack::confirm("Add another header?").interact()? {
            break;
        }
    }

    Ok(headers)
}

fn configure_builtin_extension() -> anyhow::Result<()> {
    let extensions = vec![
        (
            "autovisualiser",
            "Auto Visualiser",
            "Data visualisation and UI generation tools",
        ),
        (
            "computercontroller",
            "Computer Controller",
            "controls for webscraping, file caching, and automations",
        ),
        (
            "developer",
            "Developer Tools",
            "Code editing and shell access",
        ),
        (
            "memory",
            "Memory",
            "Tools to save and retrieve durable memories",
        ),
        (
            "tutorial",
            "Tutorial",
            "Access interactive tutorials and guides",
        ),
    ];

    let mut select = cliclack::select("Which built-in extension would you like to enable?");
    for (id, name, desc) in &extensions {
        select = select.item(id, name, desc);
    }
    let extension = select.interact()?.to_string();
    let (display_name, description) = extensions
        .iter()
        .find(|(id, _, _)| id == &extension)
        .map(|(_, name, desc)| (name.to_string(), desc.to_string()))
        .unwrap_or_else(|| (extension.clone(), extension.clone()));

    let config = if PLATFORM_EXTENSIONS.contains_key(extension.as_str()) {
        ExtensionConfig::Platform {
            name: extension.clone(),
            description,
            display_name: Some(display_name),
            bundled: Some(true),
            available_tools: Vec::new(),
        }
    } else {
        let timeout = prompt_extension_timeout()?;
        ExtensionConfig::Builtin {
            name: extension.clone(),
            display_name: Some(display_name),
            timeout: Some(timeout),
            bundled: Some(true),
            description,
            available_tools: Vec::new(),
        }
    };

    set_extension(ExtensionEntry {
        enabled: true,
        config,
    });

    cliclack::outro(format!("Enabled {} extension", style(extension).green()))?;
    Ok(())
}

fn configure_stdio_extension() -> anyhow::Result<()> {
    let name = prompt_extension_name("my-extension")?;

    let command_str: String = cliclack::input("What command should be run?")
        .placeholder("npx -y @block/gdrive")
        .validate(|input: &String| {
            if input.is_empty() {
                Err("Please enter a command")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let timeout = prompt_extension_timeout()?;

    let mut parts = goose::utils::split_command_args(&command_str)?;
    let cmd = if parts.is_empty() {
        String::new()
    } else {
        parts.remove(0)
    };
    let args = parts;

    let description = prompt_extension_description()?;
    let (envs, env_keys) = collect_env_vars()?;

    set_extension(ExtensionEntry {
        enabled: true,
        config: ExtensionConfig::Stdio {
            name: name.clone(),
            cmd,
            args,
            envs: Envs::new(envs),
            env_keys,
            description,
            timeout: Some(timeout),
            cwd: None,
            bundled: None,
            available_tools: Vec::new(),
        },
    });

    cliclack::outro(format!("Added {} extension", style(name).green()))?;
    Ok(())
}

fn configure_streamable_http_extension() -> anyhow::Result<()> {
    let name = prompt_extension_name("my-remote-extension")?;

    let uri: String = cliclack::input("What is the Streaming HTTP endpoint URI?")
        .placeholder("http://localhost:8000/messages")
        .validate(|input: &String| {
            if input.is_empty() {
                Err("Please enter a URI")
            } else if !(input.starts_with("http://") || input.starts_with("https://")) {
                Err("URI should start with http:// or https://")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let timeout = prompt_extension_timeout()?;
    let description = prompt_extension_description()?;
    let headers = collect_headers()?;

    // Original behavior: no env var collection for Streamable HTTP
    let envs = HashMap::new();
    let env_keys = Vec::new();

    set_extension(ExtensionEntry {
        enabled: true,
        config: ExtensionConfig::StreamableHttp {
            name: name.clone(),
            uri,
            envs: Envs::new(envs),
            env_keys,
            headers,
            description,
            timeout: Some(timeout),
            socket: None,
            client_id: None,
            client_secret_key: None,
            scopes: vec![],
            bundled: None,
            available_tools: Vec::new(),
        },
    });

    cliclack::outro(format!("Added {} extension", style(name).green()))?;
    Ok(())
}

pub fn configure_extensions_dialog() -> anyhow::Result<()> {
    let extension_type = cliclack::select("What type of extension would you like to add?")
        .item(
            "built-in",
            "Built-in Extension",
            "Use an extension that comes with goose",
        )
        .item(
            "stdio",
            "Command-line Extension",
            "Run a local command or script",
        )
        .item(
            "streamable_http",
            "Remote Extension (Streamable HTTP)",
            "Connect to a remote extension via MCP Streamable HTTP",
        )
        .interact()?;

    match extension_type {
        "built-in" => configure_builtin_extension()?,
        "stdio" => configure_stdio_extension()?,
        "streamable_http" => configure_streamable_http_extension()?,
        _ => unreachable!(),
    };

    print_config_file_saved()?;
    Ok(())
}

pub fn remove_extension_dialog() -> anyhow::Result<()> {
    for warning in goose::config::get_warnings() {
        eprintln!("{}", style(format!("Warning: {}", warning)).yellow());
    }

    let extensions = get_all_extensions();

    // Create a list of extension names and their enabled status
    let mut extension_status: Vec<(String, bool)> = extensions
        .iter()
        .map(|entry| (entry.config.name().to_string(), entry.enabled))
        .collect();

    // Sort extensions alphabetically by name
    extension_status.sort_by(|a, b| a.0.cmp(&b.0));

    if extensions.is_empty() {
        cliclack::outro(
            "No extensions configured yet. Run configure and add some extensions first.",
        )?;
        return Ok(());
    }

    // Check if all extensions are enabled
    if extension_status.iter().all(|(_, enabled)| *enabled) {
        cliclack::outro(
            "All extensions are currently enabled. You must first disable extensions before removing them.",
        )?;
        return Ok(());
    }

    // Filter out only disabled extensions
    let disabled_extensions: Vec<_> = extensions
        .iter()
        .filter(|entry| !entry.enabled)
        .map(|entry| (entry.config.name().to_string(), entry.enabled))
        .collect();

    let selected = cliclack::multiselect("Select extensions to remove (note: you can only remove disabled extensions - use \"space\" to toggle and \"enter\" to submit)")
        .required(false)
        .items(
            &disabled_extensions
                .iter()
                .filter(|(_, enabled)| !enabled)
                .map(|(name, _)| (name, name.as_str(), MULTISELECT_VISIBILITY_HINT))
                .collect::<Vec<_>>(),
        )
        .filter_mode()
        .interact()?;

    for name in selected {
        remove_extension(&name_to_key(name));
        PermissionManager::instance().remove_extension(&name_to_key(name));
        cliclack::outro(format!("Removed {} extension", style(name).green()))?;
    }

    print_config_file_saved()?;

    Ok(())
}

pub async fn configure_settings_dialog() -> anyhow::Result<()> {
    #[allow(unused_mut)]
    let mut setting_select = cliclack::select("What setting would you like to configure?").item(
        "goose_mode",
        "goose mode",
        "Configure goose mode",
    );
    #[cfg(feature = "telemetry")]
    {
        setting_select = setting_select.item(
            "telemetry",
            "Telemetry",
            "Enable or disable anonymous usage data collection",
        );
    }
    let setting_type = setting_select
        .item(
            "tool_permission",
            "Tool Permission",
            "Set permission for individual tool of enabled extensions",
        )
        .item(
            "tool_output",
            "Tool Output",
            "Show more or less tool output",
        )
        .item(
            "max_turns",
            "Max Turns",
            "Set maximum number of turns without user input",
        )
        .item(
            "keyring",
            "Secret Storage",
            "Configure how secrets are stored (keyring vs file)",
        )
        .item(
            "experiment",
            "Toggle Experiment",
            "Enable or disable an experiment feature",
        )
        .item(
            "recipe",
            "goose recipe github repo",
            "goose will pull recipes from this repo if not found locally.",
        )
        .interact()?;

    let mut should_print_config_path = true;

    match setting_type {
        "goose_mode" => {
            configure_goose_mode_dialog()?;
        }
        #[cfg(feature = "telemetry")]
        "telemetry" => {
            configure_telemetry_dialog()?;
        }
        "tool_permission" => {
            configure_tool_permissions_dialog().await.and(Ok(()))?;
            // No need to print config file path since it's already handled.
            should_print_config_path = false;
        }
        "tool_output" => {
            configure_tool_output_dialog()?;
        }
        "max_turns" => {
            configure_max_turns_dialog()?;
        }
        "keyring" => {
            configure_keyring_dialog()?;
        }
        "experiment" => {
            toggle_experiments_dialog()?;
        }
        "recipe" => {
            configure_recipe_dialog()?;
        }
        _ => unreachable!(),
    };

    if should_print_config_path {
        print_config_file_saved()?;
    }

    Ok(())
}

pub fn configure_goose_mode_dialog() -> anyhow::Result<()> {
    let config = Config::global();

    if std::env::var("GOOSE_MODE").is_ok() {
        let _ = cliclack::log::info(
            "Notice: GOOSE_MODE environment variable is set and will override the configuration here.",
        );
    }

    let mode = cliclack::select("Which goose mode would you like to configure?")
        .item(
            GooseMode::Auto,
            "Auto Mode",
            "Full file modification, extension usage, edit, create and delete files freely"
        )
        .item(
            GooseMode::Approve,
            "Approve Mode",
            "All tools, extensions and file modifications will require human approval"
        )
        .item(
            GooseMode::SmartApprove,
            "Smart Approve Mode",
            "Editing, creating, deleting files and using extensions will require human approval"
        )
        .item(
            GooseMode::Chat,
            "Chat Mode",
            "Engage with the selected provider without using tools, extensions, or file modification"
        )
        .interact()?;

    config.set_goose_mode(mode)?;
    let msg = match mode {
        GooseMode::Auto => "Set to Auto Mode - full file modification enabled",
        GooseMode::Approve => "Set to Approve Mode - all tools and modifications require approval",
        GooseMode::SmartApprove => "Set to Smart Approve Mode - modifications require approval",
        GooseMode::Chat => "Set to Chat Mode - no tools or modifications enabled",
    };
    cliclack::outro(msg)?;
    Ok(())
}

#[cfg(feature = "telemetry")]
pub fn configure_telemetry_dialog() -> anyhow::Result<()> {
    let config = Config::global();

    if std::env::var("GOOSE_TELEMETRY_OFF").is_ok() {
        let _ = cliclack::log::info(
            "Notice: GOOSE_TELEMETRY_OFF environment variable is set and will override the configuration here.",
        );
    }

    let current_choice = get_telemetry_choice();
    let current_status = match current_choice {
        Some(true) => "Enabled",
        Some(false) => "Disabled",
        None => "Not set",
    };

    let _ = cliclack::log::info(format!("Current telemetry status: {}", current_status));

    let enabled = cliclack::confirm("Share anonymous usage data to help improve goose?")
        .initial_value(current_choice.unwrap_or(true))
        .interact()?;

    config.set_param(TELEMETRY_ENABLED_KEY, enabled)?;

    if enabled {
        cliclack::outro("Telemetry enabled - thank you for helping improve goose!")?;
    } else {
        cliclack::outro("Telemetry disabled")?;
    }

    Ok(())
}

pub fn configure_tool_output_dialog() -> anyhow::Result<()> {
    let config = Config::global();

    if std::env::var("GOOSE_CLI_MIN_PRIORITY").is_ok() {
        let _ = cliclack::log::info(
            "Notice: GOOSE_CLI_MIN_PRIORITY environment variable is set and will override the configuration here.",
        );
    }
    let tool_log_level = cliclack::select("Which tool output would you like to show?")
        .item("high", "High Importance", "")
        .item("medium", "Medium Importance", "Ex. results of file-writes")
        .item("all", "All (default)", "Ex. shell command output")
        .interact()?;

    match tool_log_level {
        "high" => {
            config.set_param("GOOSE_CLI_MIN_PRIORITY", 0.8)?;
            cliclack::outro("Showing tool output of high importance only.")?;
        }
        "medium" => {
            config.set_param("GOOSE_CLI_MIN_PRIORITY", 0.2)?;
            cliclack::outro("Showing tool output of medium importance.")?;
        }
        "all" => {
            config.set_param("GOOSE_CLI_MIN_PRIORITY", 0.0)?;
            cliclack::outro("Showing all tool output.")?;
        }
        _ => unreachable!(),
    };

    Ok(())
}

pub fn configure_keyring_dialog() -> anyhow::Result<()> {
    let config = Config::global();

    if std::env::var("GOOSE_DISABLE_KEYRING").is_ok() {
        let _ = cliclack::log::info(
            "Notice: GOOSE_DISABLE_KEYRING environment variable is set and will override the configuration here.",
        );
    }

    let currently_disabled = config.get_param::<String>("GOOSE_DISABLE_KEYRING").is_ok();

    let current_status = if currently_disabled {
        "Disabled (using file-based storage)"
    } else {
        "Enabled (using system keyring)"
    };

    let _ = cliclack::log::info(format!("Current secret storage: {}", current_status));
    let secrets_path = Paths::config_dir().join("secrets.yaml");
    let _ = cliclack::log::warning(format!(
        "Note: Disabling the keyring stores secrets in a plain text file ({})",
        secrets_path.display()
    ));

    let storage_option = cliclack::select("How would you like to store secrets?")
        .item(
            "keyring",
            "System Keyring (recommended)",
            "Use secure system keyring for storing API keys and secrets",
        )
        .item(
            "file",
            "File-based Storage",
            "Store secrets in a local file (useful when keyring access is restricted)",
        )
        .interact()?;

    match storage_option {
        "keyring" => {
            // Set to empty string to enable keyring (absence or empty = enabled)
            config.set_param("GOOSE_DISABLE_KEYRING", Value::String("".to_string()))?;
            cliclack::outro("Secret storage set to system keyring (secure)")?;
            let _ =
                cliclack::log::info("You may need to restart goose for this change to take effect");
        }
        "file" => {
            // Set the disable flag to use file storage
            config.set_param("GOOSE_DISABLE_KEYRING", Value::String("true".to_string()))?;
            cliclack::outro(format!(
                "Secret storage set to file ({}). Keep this file secure!",
                secrets_path.display(),
            ))?;
            let _ =
                cliclack::log::info("You may need to restart goose for this change to take effect");
        }
        _ => unreachable!(),
    };

    Ok(())
}

/// Configure experiment features that can be used with goose
/// Dialog for toggling which experiments are enabled/disabled
pub fn toggle_experiments_dialog() -> anyhow::Result<()> {
    let experiments = ExperimentManager::get_all()?;

    if experiments.is_empty() {
        cliclack::outro("No experiments supported yet.")?;
        return Ok(());
    }

    // Get currently enabled experiments for the selection
    let enabled_experiments: Vec<&String> = experiments
        .iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(name, _)| name)
        .collect();

    // Let user toggle experiments
    let selected = cliclack::multiselect(
        "enable experiments: (use \"space\" to toggle and \"enter\" to submit)",
    )
    .required(false)
    .items(
        &experiments
            .iter()
            .map(|(name, _)| (name, name.as_str(), MULTISELECT_VISIBILITY_HINT))
            .collect::<Vec<_>>(),
    )
    .initial_values(enabled_experiments)
    .interact()?;

    // Update enabled status for each experiments
    for name in experiments.iter().map(|(name, _)| name) {
        ExperimentManager::set_enabled(name, selected.iter().any(|&s| s.as_str() == name))?;
    }

    cliclack::outro("Experiments settings updated successfully")?;
    Ok(())
}

pub async fn configure_tool_permissions_dialog() -> anyhow::Result<()> {
    let mut extensions: Vec<String> = get_enabled_extensions()
        .into_iter()
        .map(|ext| ext.name().clone())
        .collect();
    extensions.push("platform".to_string());

    extensions.sort();

    let selected_extension_name = cliclack::select("Choose an extension to configure tools")
        .items(
            &extensions
                .iter()
                .map(|ext| (ext.clone(), ext.clone(), ""))
                .collect::<Vec<_>>(),
        )
        .filter_mode()
        .interact()?;

    let config = Config::global();

    let provider_name: String = config
        .get_goose_provider()
        .expect("No provider configured. Please set model provider first");

    let model: String = config
        .get_goose_model()
        .expect("No model configured. Please set model first");
    let model_config = goose::model_config::model_config_from_user_config(&provider_name, &model)?;

    let agent = Agent::new();

    let session = agent
        .config
        .session_manager
        .create_session(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            "Tool Permission Configuration".to_string(),
            SessionType::Hidden,
            agent.config.goose_mode,
        )
        .await?;

    let extension_config = get_extension_by_name(&selected_extension_name);
    if let Some(config) = extension_config.as_ref() {
        agent
            .add_extension(config.clone(), &session.id)
            .await
            .unwrap_or_else(|_| {
                println!(
                    "{} Failed to check extension: {}",
                    style("Error").red().italic(),
                    config.name()
                );
            });
    } else {
        println!(
            "{} Configuration not found for extension: {}",
            style("Warning").yellow().italic(),
            selected_extension_name
        );
        return Ok(());
    }

    let extensions = extension_config.into_iter().collect::<Vec<_>>();
    let new_provider = create(&provider_name, extensions).await?;
    agent
        .update_provider(new_provider, model_config, &session.id)
        .await?;

    let permission_manager = PermissionManager::instance();
    let selected_tools = agent
        .list_tools(&session.id, Some(selected_extension_name.clone()))
        .await
        .into_iter()
        .map(|tool| {
            ToolInfo::new(
                &tool.name,
                tool.description
                    .as_ref()
                    .map(|d| d.as_ref())
                    .unwrap_or_default(),
                get_parameter_names(&tool),
                permission_manager.get_user_permission(&tool.name),
            )
        })
        .collect::<Vec<ToolInfo>>();

    let tool_name = cliclack::select("Choose a tool to update permission")
        .items(
            &selected_tools
                .iter()
                .map(|tool| {
                    let first_description = tool
                        .description
                        .split('.')
                        .next()
                        .unwrap_or("No description available")
                        .trim();
                    (tool.name.clone(), tool.name.clone(), first_description)
                })
                .collect::<Vec<_>>(),
        )
        .filter_mode()
        .interact()?;

    // Find the selected tool
    let tool = selected_tools
        .iter()
        .find(|tool| tool.name == tool_name)
        .unwrap();

    // Display tool description and current permission level
    let current_permission = match tool.permission {
        Some(PermissionLevel::AlwaysAllow) => "Always Allow",
        Some(PermissionLevel::AskBefore) => "Ask Before",
        Some(PermissionLevel::NeverAllow) => "Never Allow",
        None => "Not Set",
    };

    // Allow user to set the permission level
    let permission = cliclack::select(format!(
        "Set permission level for tool {}, current permission level: {}",
        tool.name, current_permission
    ))
    .item(
        "always_allow",
        "Always Allow",
        "Allow this tool to execute without asking",
    )
    .item(
        "ask_before",
        "Ask Before",
        "Prompt before executing this tool",
    )
    .item(
        "never_allow",
        "Never Allow",
        "Prevent this tool from executing",
    )
    .interact()?;

    let permission_label = match permission {
        "always_allow" => "Always Allow",
        "ask_before" => "Ask Before",
        "never_allow" => "Never Allow",
        _ => unreachable!(),
    };

    // Update the permission level in the configuration
    let new_permission = match permission {
        "always_allow" => PermissionLevel::AlwaysAllow,
        "ask_before" => PermissionLevel::AskBefore,
        "never_allow" => PermissionLevel::NeverAllow,
        _ => unreachable!(),
    };

    permission_manager.update_user_permission(&tool.name, new_permission);

    cliclack::outro(format!(
        "Updated permission level for tool {} to {}.",
        tool.name, permission_label
    ))?;

    cliclack::outro(format!(
        "Changes saved to {}",
        permission_manager.get_config_path().display()
    ))?;

    Ok(())
}

fn configure_recipe_dialog() -> anyhow::Result<()> {
    let key_name = GOOSE_RECIPE_GITHUB_REPO_CONFIG_KEY;
    let config = Config::global();
    let default_recipe_repo = std::env::var(key_name)
        .ok()
        .or_else(|| config.get_param(key_name).unwrap_or(None));
    let mut recipe_repo_input = cliclack::input(
        "Enter your goose recipe GitHub repo (owner/repo): eg: my_org/goose-recipes",
    )
    .required(false);
    if let Some(recipe_repo) = default_recipe_repo {
        recipe_repo_input = recipe_repo_input.default_input(&recipe_repo);
    }
    let input_value: String = recipe_repo_input.interact()?;
    if input_value.clone().trim().is_empty() {
        config.delete(key_name)?;
    } else {
        config.set_param(key_name, &input_value)?;
    }
    Ok(())
}

pub fn configure_max_turns_dialog() -> anyhow::Result<()> {
    let config = Config::global();

    let current_max_turns: u32 = config.get_param("GOOSE_MAX_TURNS").unwrap_or(1000);

    let max_turns_input: String =
        cliclack::input("Set maximum number of agent turns without user input:")
            .placeholder(&current_max_turns.to_string())
            .default_input(&current_max_turns.to_string())
            .validate(|input: &String| match input.parse::<u32>() {
                Ok(value) => {
                    if value < 1 {
                        Err("Value must be at least 1")
                    } else {
                        Ok(())
                    }
                }
                Err(_) => Err("Please enter a valid number"),
            })
            .interact()?;

    let max_turns: u32 = max_turns_input.parse()?;
    config.set_param("GOOSE_MAX_TURNS", max_turns)?;

    cliclack::outro(format!(
        "Set maximum turns to {} - goose will ask for input after {} consecutive actions",
        max_turns, max_turns
    ))?;

    Ok(())
}

/// Handle OpenRouter authentication
pub async fn handle_openrouter_auth() -> anyhow::Result<()> {
    use goose::config::{configure_openrouter, signup_openrouter::OpenRouterAuth};
    use goose::conversation::message::Message;
    use goose::providers::create;

    // Use the OpenRouter authentication flow
    let mut auth_flow = OpenRouterAuth::new()?;
    let api_key = auth_flow.complete_flow().await?;
    println!("\nAuthentication complete!");

    // Get config instance
    let config = Config::global();

    // Use the existing configure_openrouter function to set everything up
    println!("\nConfiguring OpenRouter...");
    configure_openrouter(config, api_key)?;

    println!("✓ OpenRouter configuration complete");
    println!("✓ Models configured successfully");

    // Test configuration - get the model that was configured
    println!("\nTesting configuration...");
    let configured_model: String = config.get_goose_model()?;
    let model_config =
        match goose::model_config::model_config_from_user_config("openrouter", &configured_model) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("⚠️  Invalid model configuration: {}", e);
                eprintln!("Your settings have been saved. Please check your model configuration.");
                return Ok(());
            }
        };

    match create("openrouter", Vec::new()).await {
        Ok(provider) => {
            let test_result = provider
                .complete(
                    &model_config,
                    "You are goose, an AI assistant.",
                    &[Message::user().with_text("Say 'Configuration test successful!'")],
                    &[],
                )
                .await;

            match test_result {
                Ok(_) => {
                    println!("✓ Configuration test passed!");

                    // Enable the developer extension by default if not already enabled
                    let entries = get_all_extensions();
                    let has_developer = entries
                        .iter()
                        .any(|e| e.config.name() == "developer" && e.enabled);

                    if !has_developer {
                        set_extension(ExtensionEntry {
                            enabled: true,
                            config: ExtensionConfig::Platform {
                                name: "developer".to_string(),
                                description: "Developer extension".to_string(),
                                display_name: Some(goose::config::DEFAULT_DISPLAY_NAME.to_string()),
                                bundled: Some(true),
                                available_tools: Vec::new(),
                            },
                        });
                        println!("✓ Developer extension enabled");
                    }

                    cliclack::outro("OpenRouter setup complete! You can now use goose.")?;
                }
                Err(e) => {
                    eprintln!("⚠️  Configuration test failed: {}", e);
                    eprintln!(
                        "Your settings have been saved, but there may be an issue with the connection."
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("⚠️  Failed to create provider for testing: {}", e);
            eprintln!("Your settings have been saved. Please check your configuration.");
        }
    }
    Ok(())
}

pub async fn handle_tetrate_auth() -> anyhow::Result<()> {
    let mut auth_flow = TetrateAuth::new()?;
    let api_key = auth_flow.complete_flow().await?;

    println!("\nAuthentication complete!");

    let config = Config::global();

    println!("\nConfiguring Tetrate Agent Router Service...");
    configure_tetrate(config, api_key)?;

    println!("✓ Tetrate Agent Router Service configuration complete");
    println!("✓ Models configured successfully");

    // Test configuration
    println!("\nTesting configuration...");
    let configured_model: String = config.get_goose_model()?;
    if let Err(e) = goose::model_config::model_config_from_user_config("tetrate", &configured_model)
    {
        eprintln!("⚠️  Invalid model configuration: {}", e);
        eprintln!("Your settings have been saved. Please check your model configuration.");
        return Ok(());
    }

    match create("tetrate", Vec::new()).await {
        Ok(provider) => {
            let test_result = provider.fetch_supported_models().await;

            match test_result {
                Ok(_) => {
                    println!("✓ Configuration test passed!");

                    let entries = get_all_extensions();
                    let has_developer = entries
                        .iter()
                        .any(|e| e.config.name() == "developer" && e.enabled);

                    if !has_developer {
                        set_extension(ExtensionEntry {
                            enabled: true,
                            config: ExtensionConfig::Platform {
                                name: "developer".to_string(),
                                description: "Developer extension".to_string(),
                                display_name: Some(goose::config::DEFAULT_DISPLAY_NAME.to_string()),
                                bundled: Some(true),
                                available_tools: Vec::new(),
                            },
                        });
                        println!("✓ Developer extension enabled");
                    }

                    cliclack::outro(
                        "Tetrate Agent Router Service setup complete! You can now use goose.",
                    )?;
                }
                Err(e) => {
                    eprintln!("⚠️  Configuration test failed: {}", e);
                    eprintln!(
                        "Your settings have been saved, but there may be an issue with the connection."
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("⚠️  Failed to create provider for testing: {}", e);
            eprintln!("Your settings have been saved. Please check your configuration.");
        }
    }

    Ok(())
}

/// Prompts the user to collect custom HTTP headers for a provider.
fn collect_custom_headers() -> anyhow::Result<Option<std::collections::HashMap<String, String>>> {
    let use_custom_headers = cliclack::confirm("Does this provider require custom headers?")
        .initial_value(false)
        .interact()?;

    if !use_custom_headers {
        return Ok(None);
    }

    let mut custom_headers = std::collections::HashMap::new();

    loop {
        let header_name: String = cliclack::input("Header name:")
            .placeholder("e.g., x-origin-client-id")
            .required(false)
            .interact()?;

        if header_name.is_empty() {
            break;
        }

        let header_value: String = cliclack::password(format!("Value for '{}':", header_name))
            .mask('▪')
            .interact()?;

        custom_headers.insert(header_name, header_value);

        let add_more = cliclack::confirm("Add another header?")
            .initial_value(false)
            .interact()?;

        if !add_more {
            break;
        }
    }

    if custom_headers.is_empty() {
        Ok(None)
    } else {
        Ok(Some(custom_headers))
    }
}

fn add_provider() -> anyhow::Result<()> {
    let config = Config::global();
    let provider_type = cliclack::select("What type of API is this?")
        .item(
            "openai_compatible",
            "OpenAI Compatible",
            "Uses OpenAI API format",
        )
        .item(
            "anthropic_compatible",
            "Anthropic Compatible",
            "Uses Anthropic API format",
        )
        .item(
            "ollama_compatible",
            "Ollama Compatible",
            "Uses Ollama API format",
        )
        .interact()?;

    let display_name: String = cliclack::input("What should we call this provider?")
        .placeholder("Your Provider Name")
        .validate(|input: &String| {
            if input.is_empty() {
                Err("Please enter a name")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let api_url: String = cliclack::input("Provider API URL:")
        .placeholder("https://api.example.com/v1")
        .validate(|input: &String| {
            if !input.starts_with("http://") && !input.starts_with("https://") {
                Err("URL must start with either http:// or https://")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let requires_auth = cliclack::confirm("Does this provider require authentication?")
        .initial_value(true)
        .interact()?;

    let mut api_key = String::new();
    let mut auth: Option<AuthConfig> = None;

    if requires_auth {
        let auth_mode = cliclack::select("How should goose obtain credentials for this provider?")
            .item("static", "Static API key", "Enter a fixed API key now")
            .item(
                "command",
                "Command (refreshable)",
                "Run a command to fetch/refresh a short-lived credential",
            )
            .interact()?;

        if auth_mode == "command" {
            let command: String = cliclack::input("Command to run for the credential:")
                .placeholder("/path/to/get-token.sh")
                .validate(|input: &String| {
                    if input.trim().is_empty() {
                        Err("Please enter a command")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;

            let args_input: String =
                cliclack::input("Command arguments (comma-separated, optional):")
                    .placeholder("--flag, value")
                    .required(false)
                    .interact()?;
            let args: Vec<String> = args_input
                .split(',')
                .map(str::trim)
                .filter(|arg| !arg.is_empty())
                .map(str::to_string)
                .collect();

            let refresh_interval_input: String = cliclack::input(
                "Refresh interval in seconds (0 to only refresh after an auth failure):",
            )
            .default_input("3600")
            .validate(|input: &String| {
                input
                    .trim()
                    .parse::<u64>()
                    .map(|_| ())
                    .map_err(|_| "Please enter a whole number of seconds")
            })
            .interact()?;
            let refresh_interval: u64 = refresh_interval_input.trim().parse().unwrap_or(3600);

            auth = Some(AuthConfig {
                command,
                args,
                refresh_interval,
                timeout_seconds: None,
                cwd: None,
            });
        } else {
            api_key = cliclack::password("API key:").mask('▪').interact()?;
        }
    }

    let models_input: String = cliclack::input("Available models (separate with commas):")
        .placeholder("model-a, model-b, model-c")
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Please enter at least one model name")
            } else {
                Ok(())
            }
        })
        .interact()?;

    let models: Vec<goose_providers::base::ModelInfo> = models_input
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(goose_providers::base::ModelInfo::new)
        .collect();

    let supports_streaming = cliclack::confirm("Does this provider support streaming responses?")
        .initial_value(true)
        .interact()?;

    let base_path_input: String = cliclack::input("API base path (optional, press Enter to skip):")
        .placeholder("e.g., v1/chat/completions or project_id/v1")
        .required(false)
        .interact()?;

    let base_path = if base_path_input.trim().is_empty() {
        None
    } else {
        Some(base_path_input)
    };

    let headers = collect_custom_headers()?;

    let provider_config = create_custom_provider(CreateCustomProviderParams {
        engine: provider_type.to_string(),
        display_name: display_name.clone(),
        api_url,
        api_key: requires_auth.then_some(api_key),
        models,
        supports_streaming: Some(supports_streaming),
        headers,
        requires_auth,
        catalog_provider_id: None,
        base_path,
        toolshim: false,
        preserves_thinking: None,
        auth,
    })?;

    if !provider_config.models.is_empty() {
        let model_items: Vec<_> = provider_config
            .models
            .iter()
            .map(|m| (m.name.as_str(), m.name.as_str(), ""))
            .collect();
        if let Ok(model) = cliclack::select("Which model should be the default?")
            .items(&model_items)
            .interact()
        {
            config.set_goose_provider(&provider_config.name)?;
            config.set_goose_model(model)?;
        }
    }

    cliclack::outro(format!("Custom provider added: {}", display_name))?;
    Ok(())
}

async fn remove_provider() -> anyhow::Result<()> {
    let custom_providers_dir = goose::config::declarative_providers::custom_providers_dir();
    let custom_providers = if custom_providers_dir.exists() {
        goose::config::declarative_providers::load_custom_providers(&custom_providers_dir)?
    } else {
        Vec::new()
    };

    if custom_providers.is_empty() {
        cliclack::outro("No custom providers added just yet.")?;
        return Ok(());
    }

    let provider_items: Vec<_> = custom_providers
        .iter()
        .map(|p| (p.name.as_str(), p.display_name.as_str(), "Custom provider"))
        .collect();

    let selected_id = cliclack::select("Which custom provider would you like to remove?")
        .items(&provider_items)
        .filter_mode()
        .interact()?;

    // Clean up provider-specific cache files (e.g., OAuth tokens) before removing config
    if let Err(e) = goose::providers::cleanup_provider(selected_id).await {
        tracing::warn!("Failed to clean up provider cache: {}", e);
    }

    remove_custom_provider(selected_id)?;
    cliclack::outro(format!("Removed custom provider: {}", selected_id))?;
    Ok(())
}

pub async fn configure_custom_provider_dialog() -> anyhow::Result<()> {
    let action = cliclack::select("What would you like to do?")
        .item(
            "add",
            "Add A Custom Provider",
            "Add a new OpenAI/Anthropic/Ollama compatible Provider",
        )
        .item(
            "remove",
            "Remove Custom Provider",
            "Remove an existing custom provider",
        )
        .interact()?;

    match action {
        "add" => add_provider(),
        "remove" => remove_provider().await,
        _ => unreachable!(),
    }?;

    print_config_file_saved()?;

    Ok(())
}

fn print_config_file_saved() -> anyhow::Result<()> {
    let config = Config::global();
    cliclack::outro(format!(
        "Configuration saved successfully to {}",
        config.path()
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_item_inside_visible_window_keeps_order() {
        let mut items: Vec<_> = (0..MAX_PROVIDER_ROWS + 1).collect();
        let expected = items.clone();

        move_selected_item_into_view(
            &mut items,
            Some(MAX_PROVIDER_ROWS - 2),
            MAX_PROVIDER_ROWS - 1,
        );

        assert_eq!(items, expected);
    }

    #[test]
    fn selected_item_outside_visible_window_moves_to_front() {
        let mut items: Vec<_> = (0..MAX_PROVIDER_ROWS + 2).collect();

        move_selected_item_into_view(
            &mut items,
            Some(MAX_PROVIDER_ROWS - 1),
            MAX_PROVIDER_ROWS - 1,
        );

        assert_eq!(items[0], MAX_PROVIDER_ROWS - 1);
        assert_eq!(
            items[1..MAX_PROVIDER_ROWS],
            (0..MAX_PROVIDER_ROWS - 1).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fuzzy_provider_filter_keeps_relevant_matches_ranked_first() {
        let items = vec![
            (
                "anthropic".to_string(),
                "Anthropic".to_string(),
                String::new(),
            ),
            (
                "openrouter".to_string(),
                "OpenRouter".to_string(),
                String::new(),
            ),
            ("openai".to_string(), "OpenAI".to_string(), String::new()),
        ];

        let filtered = fuzzy_filter_provider_items(&items, "open ai");

        assert_eq!(filtered.first().map(|item| item.0.as_str()), Some("openai"));
    }
}
