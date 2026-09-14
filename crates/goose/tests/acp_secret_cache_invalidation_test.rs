#![recursion_limit = "256"]
#[allow(dead_code)]
#[path = "acp_common_tests/mod.rs"]
mod common_tests;

use common_tests::fixtures::server::AcpServerConnection;
use common_tests::fixtures::{run_test, send_custom, Connection, TestConnectionConfig};
use goose::config::paths::Paths;
use goose::config::{Config, ConfigError};
use goose::providers::base::{MessageStream, Provider};
use goose::providers::inventory::ProviderInventoryService;
use goose::session::session_manager::SessionStorage;
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use goose_test_support::EnforceSessionId;
use serial_test::serial;
use std::sync::Arc;

struct MockProvider {
    name: String,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    async fn stream(
        &self,
        _model_config: &ModelConfig,
        _system: &str,
        _messages: &[goose::conversation::message::Message],
        _tools: &[rmcp::model::Tool],
    ) -> Result<MessageStream, ProviderError> {
        unimplemented!()
    }

    async fn fetch_recommended_models(
        &self,
        _toolshim: bool,
    ) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["claude-3-5-haiku-latest".to_string()])
    }
}

fn mock_provider_factory() -> goose::acp::server::AcpProviderFactory {
    Arc::new(
        |provider_name, _extensions, _working_dir, _use_default_model| {
            Box::pin(async move {
                Ok(Arc::new(MockProvider {
                    name: provider_name,
                }) as Arc<dyn Provider>)
            })
        },
    )
}

fn write_config(config_dir: &std::path::Path) {
    std::fs::create_dir_all(config_dir).unwrap();
    std::fs::write(
        config_dir.join(goose::config::base::CONFIG_YAML_NAME),
        "GOOSE_MODEL: gpt-4o\nGOOSE_PROVIDER: openai\nGOOSE_DISABLE_KEYRING: true\n",
    )
    .unwrap();
}

fn write_secrets(config_dir: &std::path::Path, secrets: &str) {
    std::fs::write(config_dir.join("secrets.yaml"), secrets).unwrap();
}

#[test]
#[serial]
fn provider_secret_mutations_and_inventory_refresh_invalidate_global_secret_cache() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_string_lossy().to_string();
    let _env = env_lock::lock_env([
        ("GOOSE_PATH_ROOT", Some(root_path.as_str())),
        ("GOOSE_DISABLE_KEYRING", Some("1")),
        ("ANTHROPIC_API_KEY", None),
        ("OPENAI_API_KEY", None),
        ("XAI_API_KEY", None),
        ("XAI_HOST", None),
    ]);

    let config_dir = Paths::config_dir();
    let data_dir = Paths::data_dir();
    write_config(&config_dir);
    run_test(async move {
        let openai = common_tests::fixtures::OpenAiFixture::new(
            vec![],
            Arc::new(EnforceSessionId::default()),
        )
        .await;
        let config = TestConnectionConfig {
            data_root: config_dir.clone(),
            provider_factory: Some(mock_provider_factory()),
            ..Default::default()
        };
        let conn = AcpServerConnection::new(config, openai).await;

        let save_provider_config = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/save",
            serde_json::json!({
                "providerId": "xai",
                "fields": [
                    {
                        "key": "XAI_API_KEY",
                        "value": "xai-provider-config-key",
                    },
                    {
                        "key": "XAI_HOST",
                        "value": "https://api.x.ai/v1",
                    },
                ],
            }),
        )
        .await
        .expect("provider config save should succeed");
        assert_eq!(
            save_provider_config.get("status"),
            Some(&serde_json::json!({
                "providerId": "xai",
                "isConfigured": true,
            })),
            "provider config save should return the updated configured status"
        );
        assert_eq!(
            save_provider_config.get("refresh"),
            Some(&serde_json::json!({
                "started": ["xai"],
                "skipped": [],
            })),
            "provider config save should return the inventory refresh acknowledgement"
        );
        assert_eq!(
            Config::global()
                .get_secret::<String>("XAI_API_KEY")
                .unwrap(),
            "xai-provider-config-key",
            "provider config save should invalidate the global secrets cache"
        );

        let read_provider_config = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/read",
            serde_json::json!({
                "providerId": "xai",
            }),
        )
        .await
        .expect("provider config read should succeed");
        let fields = read_provider_config
            .get("fields")
            .and_then(|fields| fields.as_array())
            .expect("provider config read should return fields");
        let api_key_field = fields
            .iter()
            .find(|field| field.get("key") == Some(&serde_json::json!("XAI_API_KEY")))
            .expect("provider config read should include the API key");
        assert_eq!(api_key_field.get("isSet"), Some(&serde_json::json!(true)));
        assert_ne!(
            api_key_field.get("value"),
            Some(&serde_json::json!("xai-provider-config-key")),
            "provider config read should mask secret values"
        );

        let delete_provider_config = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/delete",
            serde_json::json!({
                "providerId": "xai",
            }),
        )
        .await
        .expect("provider config delete should succeed");
        assert_eq!(
            delete_provider_config.get("status"),
            Some(&serde_json::json!({
                "providerId": "xai",
                "isConfigured": false,
            })),
            "provider config delete should return the updated configured status"
        );
        assert!(
            matches!(
                Config::global().get_secret::<String>("XAI_API_KEY"),
                Err(ConfigError::NotFound(_))
            ),
            "provider config delete should invalidate the global secrets cache"
        );

        Config::global().invalidate_secrets_cache();
        assert!(Config::global()
            .get_secret::<String>("ANTHROPIC_API_KEY")
            .is_err());

        write_secrets(&config_dir, "ANTHROPIC_API_KEY: anthropic-key\n");

        let refresh = send_custom(
            conn.cx(),
            "_goose/unstable/providers/inventory/refresh",
            serde_json::json!({
                "providerIds": ["anthropic"],
            }),
        )
        .await
        .expect("inventory refresh should succeed");

        assert_eq!(
            refresh.get("started"),
            Some(&serde_json::json!(["anthropic"])),
            "inventory refresh should invalidate the global secrets cache before planning"
        );

        write_secrets(&config_dir, "OPENAI_API_KEY: plan-time-key\n");
        Config::global().invalidate_secrets_cache();

        let inventory = ProviderInventoryService::new(Arc::new(SessionStorage::new(data_dir)));
        let plan = inventory
            .plan_refresh(&["openai".to_string()])
            .await
            .expect("plan refresh should start for configured OpenAI provider");
        assert_eq!(plan.started, vec!["openai".to_string()]);

        let entry_during_refresh = inventory
            .entry_for_provider("openai")
            .await
            .expect("entry should load while refresh is in progress")
            .expect("OpenAI inventory entry should exist");
        assert!(
            entry_during_refresh.refreshing,
            "plan refresh should mark the plan-time identity as refreshing"
        );

        let sentinel_model = "stark-plan-time-model".to_string();
        inventory
            .store_refreshed_models("openai", std::slice::from_ref(&sentinel_model))
            .await
            .expect("public store_refreshed_models compatibility wrapper should succeed");

        let plan_time_entry = inventory
            .entry_for_provider("openai")
            .await
            .expect("entry should load for plan-time credentials")
            .expect("OpenAI inventory entry should exist for plan-time credentials");
        assert!(
            !plan_time_entry.refreshing,
            "store with captured identity should clear the plan-time refreshing key"
        );
        assert!(
            plan_time_entry
                .models
                .iter()
                .any(|model| model.id == sentinel_model),
            "models should be stored under the identity captured at plan time"
        );
    });
}
