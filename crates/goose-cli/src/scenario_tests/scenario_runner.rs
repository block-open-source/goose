use dotenvy::dotenv;
use goose::conversation::Conversation;

use crate::scenario_tests::message_generator::MessageGenerator;
use crate::scenario_tests::mock_client::weather_client;
use crate::scenario_tests::provider_configs::{get_provider_configs, ProviderConfig};
use crate::session::CliSession;
use anyhow::Result;
use goose::agents::{Agent, AgentConfig, GoosePlatform};
use goose::config::permission::PermissionManager;
use goose::config::GooseMode;
use goose::providers::{create, testprovider::TestProvider};
use goose::session::session_manager::SessionType;
use goose::session::SessionManager;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

pub const SCENARIO_TESTS_DIR: &str = "src/scenario_tests";

#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub messages: Conversation,
    pub error: Option<String>,
}

impl ScenarioResult {
    pub fn message_contents(&self) -> Vec<String> {
        self.messages
            .iter()
            .flat_map(|msg| &msg.content)
            .map(|content| content.as_text().unwrap_or("").to_string())
            .collect()
    }

    pub fn last_message(&self) -> Result<String, anyhow::Error> {
        let message_contents = self.message_contents();
        message_contents
            .last()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No messages found in scenario result"))
    }
}

pub async fn run_scenario<F>(
    test_name: &str,
    message_generator: MessageGenerator<'_>,
    providers_to_skip: Option<&[&str]>,
    validator: F,
) -> Result<()>
where
    F: Fn(&ScenarioResult) -> Result<()> + Send + Sync + 'static,
{
    if let Ok(only_provider) = std::env::var("GOOSE_TEST_PROVIDER") {
        let active_providers = get_provider_configs();
        let config = active_providers
            .iter()
            .find(|c| c.name.to_lowercase() == only_provider.to_lowercase())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Provider '{}' not found. Available: {}",
                    only_provider,
                    get_provider_configs()
                        .iter()
                        .map(|c| c.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;

        println!("Running test '{}' for provider: {}", test_name, config.name);
        run_provider_scenario_with_validation(config, test_name, &message_generator, &validator)
            .await?;
        return Ok(());
    }

    let excluded_providers: HashSet<_> = providers_to_skip
        .into_iter()
        .flatten()
        .map(|name| name.to_lowercase())
        .collect();

    let all_configs = get_provider_configs();

    let all_config_len = all_configs.len();

    let configs_to_test: Vec<_> = all_configs
        .into_iter()
        .filter(|c| !excluded_providers.contains(&c.name.to_lowercase()))
        .collect();

    if let Some(to_skip) = providers_to_skip {
        if configs_to_test.len() != all_config_len - to_skip.len() {
            return Err(anyhow::anyhow!("Some providers in skip list don't exist"));
        }
    }

    let mut failures = Vec::new();

    for config in configs_to_test {
        match run_provider_scenario_with_validation(
            config,
            test_name,
            &message_generator,
            &validator,
        )
        .await
        {
            Ok(_) => println!("✅ {} - {}", test_name, config.name),
            Err(e) => {
                println!("❌ {} - {} FAILED: {}", test_name, config.name, e);
                failures.push((config.name, e));
            }
        }
    }

    if !failures.is_empty() {
        println!("\n=== Test Failures for {} ===", test_name);
        for (provider, error) in &failures {
            println!("❌ {}: {}", provider, error);
        }
        return Err(anyhow::anyhow!(
            "Test '{}' failed for {} provider(s)",
            test_name,
            failures.len()
        ));
    }

    Ok(())
}

async fn run_provider_scenario_with_validation<F>(
    config: &ProviderConfig,
    test_name: &str,
    message_generator: &MessageGenerator<'_>,
    validator: &F,
) -> Result<()>
where
    F: Fn(&ScenarioResult) -> Result<()>,
{
    use goose::config::ExtensionConfig;

    goose::agents::moim::SKIP.with(|f| f.set(true));

    if let Ok(path) = dotenv() {
        println!("Loaded environment from {:?}", path);
    }

    let factory_name = config.name.to_lowercase();
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let file_path = format!(
        "{}/{}/recordings/{}/{}.json",
        manifest_dir,
        SCENARIO_TESTS_DIR,
        factory_name.to_lowercase(),
        test_name
    );

    if let Some(parent) = Path::new(&file_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let replay_mode = Path::new(&file_path).exists();
    let (provider_arc, provider_for_saving, original_env) = if replay_mode {
        match TestProvider::new_replaying(&file_path) {
            Ok(test_provider) => (Arc::new(test_provider), None, None),
            Err(e) => {
                let _ = std::fs::remove_file(&file_path);
                return Err(anyhow::anyhow!(
                    "Test replay failed for '{}' ({}): {}. File deleted - re-run test to record fresh data.",
                    test_name, factory_name, e
                ));
            }
        }
    } else {
        if std::env::var("GITHUB_ACTIONS").is_ok() {
            panic!(
                "Test recording is not supported on CI. \
            Did you forget to add the file {} to the repository and were expecting that to replay?",
                file_path
            );
        }

        let original_env = setup_environment(config)?;

        let inner_provider = create(&factory_name, Vec::new()).await?;

        let test_provider = Arc::new(TestProvider::new_recording(inner_provider, &file_path));
        (
            test_provider.clone(),
            Some(test_provider),
            Some(original_env),
        )
    };

    let messages = vec![message_generator(&*provider_arc)];

    let mock_client = weather_client();

    let temp_dir = TempDir::new()?;
    let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));
    let permission_manager = Arc::new(PermissionManager::new(temp_dir.path().to_path_buf()));
    let agent_config = AgentConfig::new(
        session_manager,
        permission_manager,
        None,
        GooseMode::Auto,
        true,
        GoosePlatform::GooseCli,
    );
    let agent = Agent::with_config(agent_config);
    agent
        .extension_manager
        .add_client(
            "weather_extension".to_string(),
            ExtensionConfig::Builtin {
                name: "".to_string(),
                display_name: None,
                description: "".to_string(),
                timeout: None,
                bundled: None,
                available_tools: vec![],
            },
            Arc::new(mock_client),
            None,
        )
        .await;

    let session = agent
        .config
        .session_manager
        .create_session(
            PathBuf::default(),
            "scenario-runner".to_string(),
            SessionType::Hidden,
            GooseMode::default(),
        )
        .await?;

    let scenario_model_config =
        goose::model_config::model_config_from_user_config(&factory_name, config.model_name)?;
    agent
        .update_provider(
            provider_arc as Arc<dyn goose::providers::base::Provider>,
            scenario_model_config,
            &session.id,
        )
        .await?;

    let mut cli_session = CliSession::new(
        Arc::new(agent),
        session.id,
        false,
        None,
        None,
        None,
        None,
        "text".to_string(),
        false,
        false,
        None,
    )
    .await;

    let mut error = None;
    for message in &messages {
        if let Err(e) = cli_session
            .process_message(message.clone(), CancellationToken::default(), false)
            .await
        {
            error = Some(e.to_string());
            break;
        }
    }
    let updated_messages = cli_session.message_history();

    if let Some(ref err_msg) = error {
        if err_msg.contains("No recorded response found") {
            let _ = std::fs::remove_file(&file_path);
            return Err(anyhow::anyhow!(
                "Test replay failed for '{}' ({}) - missing recorded interaction: {}. File deleted - re-run test to record fresh data.",
                test_name, factory_name, err_msg
            ));
        }
    }

    let result = ScenarioResult {
        messages: updated_messages,
        error,
    };

    validator(&result)?;

    drop(cli_session);

    if let Some(provider) = provider_for_saving {
        if result.error.is_none() {
            Arc::try_unwrap(provider)
                .map_err(|_| anyhow::anyhow!("Failed to unwrap provider for recording"))?
                .finish_recording()?;
        }
    }

    if let Some(env) = original_env {
        restore_environment(config, &env);
    }

    Ok(())
}

fn setup_environment(config: &ProviderConfig) -> Result<HashMap<&'static str, String>> {
    let mut original_env = HashMap::new();

    for &var in config.required_env_vars {
        if let Ok(val) = std::env::var(var) {
            original_env.insert(var, val);
        }
    }

    if let Some(mods) = &config.env_modifications {
        for &var in mods.keys() {
            if let Ok(val) = std::env::var(var) {
                original_env.insert(var, val);
            }
        }
    }

    if let Some(mods) = &config.env_modifications {
        for (&var, value) in mods.iter() {
            match value {
                Some(val) => std::env::set_var(var, val),
                None => std::env::remove_var(var),
            }
        }
    }

    let missing_vars = config
        .required_env_vars
        .iter()
        .any(|var| std::env::var(var).is_err());

    if missing_vars {
        println!(
            "Skipping {} scenario - credentials not configured",
            config.name
        );
        return Err(anyhow::anyhow!("Missing required environment variables"));
    }

    Ok(original_env)
}

fn restore_environment(config: &ProviderConfig, original_env: &HashMap<&'static str, String>) {
    for (&var, value) in original_env.iter() {
        std::env::set_var(var, value);
    }
    if let Some(mods) = &config.env_modifications {
        for &var in mods.keys() {
            if !original_env.contains_key(var) {
                std::env::remove_var(var);
            }
        }
    }
}
