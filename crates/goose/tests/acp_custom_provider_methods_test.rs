#![recursion_limit = "256"]
#[allow(dead_code)]
#[path = "acp_common_tests/mod.rs"]
mod common_tests;

use common_tests::fixtures::server::AcpServerConnection;
use common_tests::fixtures::{run_test, send_custom, Connection, TestConnectionConfig};
use goose::config::base::CONFIG_YAML_NAME;
use goose::config::declarative_providers::load_provider;
use goose::config::paths::Paths;
use goose::config::{Config, ConfigError, DeclarativeProviderConfig};
use goose_test_support::EnforceSessionId;
use serial_test::serial;
use std::sync::Arc;

fn write_config(config_dir: &std::path::Path, contents: &str) {
    std::fs::create_dir_all(config_dir).unwrap();
    std::fs::write(config_dir.join(CONFIG_YAML_NAME), contents).unwrap();
}

fn write_secrets(config_dir: &std::path::Path, contents: &str) {
    std::fs::write(config_dir.join("secrets.yaml"), contents).unwrap();
}

#[test]
#[serial]
fn acp_catalog_and_custom_provider_methods_use_core_provider_store() {
    let _env = env_lock::lock_env([
        ("GOOSE_DISABLE_KEYRING", Some("1")),
        ("XAI_API_KEY", None),
        ("XAI_HOST", None),
        ("CUSTOM_STARK_ACP_PROVIDER_API_KEY", None),
    ]);

    run_test(async move {
        let config_dir = Paths::config_dir();
        write_config(
            &config_dir,
            "GOOSE_MODEL: gpt-4o\nGOOSE_PROVIDER: openai\nGOOSE_DISABLE_KEYRING: true\nXAI_HOST: https://api.x.ai/v1\n",
        );
        write_secrets(&config_dir, "XAI_API_KEY: xai-configured-key\n");
        Config::global().invalidate_secrets_cache();

        let openai = common_tests::fixtures::OpenAiFixture::new(
            vec![],
            Arc::new(EnforceSessionId::default()),
        )
        .await;
        let config = TestConnectionConfig {
            data_root: config_dir.clone(),
            ..Default::default()
        };
        let conn = AcpServerConnection::new(config, openai).await;

        let catalog = send_custom(
            conn.cx(),
            "_goose/unstable/providers/catalog/list",
            serde_json::json!({ "format": "openai" }),
        )
        .await
        .expect("provider catalog list should succeed");
        let catalog_providers = catalog
            .get("providers")
            .and_then(|providers| providers.as_array())
            .expect("catalog response should include providers");
        assert!(
            catalog_providers
                .iter()
                .any(|provider| provider.get("providerId") == Some(&serde_json::json!("opencode"))),
            "OpenAI-compatible catalog should include OpenCode Zen"
        );

        let setup_catalog = send_custom(
            conn.cx(),
            "_goose/unstable/providers/setup/catalog/list",
            serde_json::json!({}),
        )
        .await
        .expect("provider setup catalog list should succeed");
        let setup_providers = setup_catalog
            .get("providers")
            .and_then(|providers| providers.as_array())
            .expect("setup catalog response should include providers");
        for provider_id in [
            "goose",
            "anthropic",
            "openai",
            "claude-acp",
            "codex-acp",
            "copilot-acp",
            "amp-acp",
            "cursor-agent",
            "pi-acp",
        ] {
            assert!(
                setup_providers
                    .iter()
                    .any(|provider| provider.get("providerId")
                        == Some(&serde_json::json!(provider_id))),
                "setup catalog should include {provider_id}"
            );
        }
        for provider_id in ["codex", "claude-code", "gemini-cli"] {
            assert!(
                setup_providers
                    .iter()
                    .all(|provider| provider.get("providerId")
                        != Some(&serde_json::json!(provider_id))),
                "setup catalog should exclude deprecated provider {provider_id}"
            );
        }
        let inventory = send_custom(
            conn.cx(),
            "_goose/unstable/providers/list",
            serde_json::json!({}),
        )
        .await
        .expect("provider inventory list should succeed");
        let inventory_providers = inventory
            .get("entries")
            .and_then(|entries| entries.as_array())
            .expect("provider inventory response should include entries");
        let deprecated = inventory_providers
            .iter()
            .find(|provider| provider.get("providerId") == Some(&serde_json::json!("claude-code")))
            .expect("complete inventory should retain deprecated providers");
        assert_eq!(deprecated.get("deprecated"), Some(&serde_json::json!(true)));
        assert_eq!(
            deprecated.get("visibleInSetup"),
            Some(&serde_json::json!(false))
        );
        assert!(
            setup_providers
                .iter()
                .all(|provider| provider.get("providerId")
                    != Some(&serde_json::json!("claude-code"))),
            "setup catalog should omit deprecated providers"
        );
        assert!(
            inventory_providers.iter().any(
                |provider| provider.get("providerId") == Some(&serde_json::json!("claude-acp"))
            ),
            "complete inventory should include replacement providers"
        );
        assert!(
            setup_providers.iter().any(
                |provider| provider.get("providerId") == Some(&serde_json::json!("claude-acp"))
            ),
            "setup catalog should include replacement providers"
        );
        let codex_setup = setup_providers
            .iter()
            .find(|provider| provider.get("providerId") == Some(&serde_json::json!("codex-acp")))
            .expect("setup catalog should include codex-acp");
        assert_eq!(
            codex_setup.get("category"),
            Some(&serde_json::json!("agent"))
        );
        assert_eq!(
            codex_setup.get("setupMethod"),
            Some(&serde_json::json!("cli_auth"))
        );
        assert_eq!(
            codex_setup.get("supportsInstall"),
            Some(&serde_json::json!(true))
        );
        assert!(
            codex_setup
                .get("aliases")
                .and_then(|aliases| aliases.as_array())
                .is_some_and(|aliases| aliases.contains(&serde_json::json!("codex"))),
            "codex-acp setup aliases should include codex"
        );

        let template = send_custom(
            conn.cx(),
            "_goose/unstable/providers/catalog/template",
            serde_json::json!({ "providerId": "zai" }),
        )
        .await
        .expect("provider catalog template should succeed");
        assert_eq!(
            template.pointer("/template/providerId"),
            Some(&serde_json::json!("zai"))
        );
        assert!(
            template
                .pointer("/template/models")
                .and_then(|models| models.as_array())
                .is_some_and(|models| !models.is_empty()),
            "provider template should expose model templates"
        );

        let configured_status = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/status",
            serde_json::json!({ "providerIds": ["xai"] }),
        )
        .await
        .expect("provider config status should succeed");
        assert_eq!(
            configured_status.pointer("/statuses/0"),
            Some(&serde_json::json!({
                "providerId": "xai",
                "isConfigured": true,
            })),
            "provider configured through core config should be configured through ACP"
        );

        let configured_read = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/read",
            serde_json::json!({ "providerId": "xai" }),
        )
        .await
        .expect("provider config read should succeed");
        let fields = configured_read
            .get("fields")
            .and_then(|fields| fields.as_array())
            .expect("provider config read should include fields");
        let xai_key = fields
            .iter()
            .find(|field| field.get("key") == Some(&serde_json::json!("XAI_API_KEY")))
            .expect("provider config read should include XAI_API_KEY");
        assert_eq!(xai_key.get("isSet"), Some(&serde_json::json!(true)));
        assert_ne!(
            xai_key.get("value"),
            Some(&serde_json::json!("xai-configured-key")),
            "provider config read should not expose raw secret values"
        );

        let non_oauth_auth = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/authenticate",
            serde_json::json!({ "providerId": "xai" }),
        )
        .await;
        assert!(
            non_oauth_auth.is_err(),
            "native auth should reject providers without an OAuth flow"
        );

        Config::global().invalidate_secrets_cache();
        assert!(Config::global()
            .get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY")
            .is_err());

        let created = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/create",
            serde_json::json!({
                "engine": "openai_compatible",
                "displayName": "Stark ACP Provider",
                "apiUrl": "https://stark.example/v1",
                "apiKey": "created-custom-key",
                "models": ["stark-1", "stark-2"],
                "supportsStreaming": true,
                "headers": {
                    "X-Stark": "enabled"
                },
                "requiresAuth": true,
                "catalogProviderId": "openai",
                "basePath": "v1/chat/completions",
                "toolshim": true
            }),
        )
        .await
        .expect("custom provider create should succeed");
        let provider_id = created
            .get("providerId")
            .and_then(|provider_id| provider_id.as_str())
            .expect("custom provider create should return providerId")
            .to_string();
        assert_eq!(provider_id, "custom_stark_acp_provider");
        assert_eq!(
            created.get("status"),
            Some(&serde_json::json!({
                "providerId": provider_id,
                "isConfigured": true,
            })),
            "create should invalidate the secret cache before status checks"
        );
        assert_eq!(
            created.get("refresh"),
            Some(&serde_json::json!({
                "started": [],
                "skipped": [
                    {
                        "providerId": provider_id,
                        "reason": "does_not_support_refresh",
                    },
                ],
            }))
        );

        let custom_provider_path = Paths::config_dir()
            .join("custom_providers")
            .join(format!("{provider_id}.json"));
        assert!(
            custom_provider_path.exists(),
            "custom provider should be saved in Goose's declarative provider store"
        );
        let saved_provider: DeclarativeProviderConfig =
            serde_json::from_str(&std::fs::read_to_string(&custom_provider_path).unwrap())
                .expect("saved provider should be core-compatible declarative config");
        assert_eq!(saved_provider.name, provider_id);
        assert_eq!(saved_provider.display_name, "Stark ACP Provider");
        assert_eq!(saved_provider.base_url, "https://stark.example/v1");
        assert!(saved_provider.toolshim);
        assert!(saved_provider.preserves_thinking);
        assert_eq!(
            saved_provider
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["stark-1", "stark-2"]
        );
        assert_eq!(
            Config::global()
                .get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY")
                .unwrap(),
            "created-custom-key",
            "custom provider create should write through Goose's config store"
        );
        assert!(
            load_provider(&provider_id)
                .expect("core should load the ACP-created custom provider")
                .is_editable
        );

        let read = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/read",
            serde_json::json!({ "providerId": provider_id }),
        )
        .await
        .expect("custom provider read should succeed");
        assert_eq!(read.get("editable"), Some(&serde_json::json!(true)));
        assert_eq!(
            read.pointer("/provider"),
            Some(&serde_json::json!({
                "providerId": provider_id,
                "engine": "openai_compatible",
                "displayName": "Stark ACP Provider",
                "apiUrl": "https://stark.example/v1",
                "models": ["stark-1", "stark-2"],
                "supportsStreaming": true,
                "headers": {
                    "X-Stark": "enabled"
                },
                "requiresAuth": true,
                "catalogProviderId": "openai",
                "basePath": "v1/chat/completions",
                "toolshim": true,
                "apiKeyEnv": "CUSTOM_STARK_ACP_PROVIDER_API_KEY",
                "apiKeySet": true,
                "preservesThinking": true,
            }))
        );

        let inventory = send_custom(
            conn.cx(),
            "_goose/unstable/providers/list",
            serde_json::json!({ "providerIds": [provider_id] }),
        )
        .await
        .expect("provider inventory list should include custom provider");
        assert_eq!(
            inventory.pointer("/entries/0/providerType"),
            Some(&serde_json::json!("Custom"))
        );
        assert_eq!(
            inventory.pointer("/entries/0/providerId"),
            Some(&serde_json::json!(provider_id))
        );

        let updated = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/update",
            serde_json::json!({
                "providerId": provider_id,
                "engine": "openai",
                "displayName": "Stark ACP Provider Updated",
                "apiUrl": "https://stark.example/openai",
                "apiKey": "updated-custom-key",
                "models": ["stark-3"],
                "supportsStreaming": false,
                "headers": {},
                "requiresAuth": true,
                "catalogProviderId": "zai",
                "toolshim": true,
                "preservesThinking": false
            }),
        )
        .await
        .expect("custom provider update should succeed");
        assert_eq!(
            updated.get("status"),
            Some(&serde_json::json!({
                "providerId": provider_id,
                "isConfigured": true,
            })),
            "update should invalidate the secret cache before status checks"
        );
        assert_eq!(
            Config::global()
                .get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY")
                .unwrap(),
            "updated-custom-key",
            "custom provider update should write through Goose's config store"
        );
        let updated_provider: DeclarativeProviderConfig =
            serde_json::from_str(&std::fs::read_to_string(&custom_provider_path).unwrap())
                .expect("updated provider should remain core-compatible");
        assert_eq!(updated_provider.display_name, "Stark ACP Provider Updated");
        assert_eq!(updated_provider.base_url, "https://stark.example/openai");
        assert_eq!(
            updated_provider.catalog_provider_id,
            Some("zai".to_string())
        );
        assert_eq!(updated_provider.base_path, None);
        assert_eq!(updated_provider.headers, None);
        assert!(updated_provider.toolshim);
        assert!(!updated_provider.preserves_thinking);
        assert_eq!(
            updated_provider
                .models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["stark-3"]
        );

        let auth_disabled = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/update",
            serde_json::json!({
                "providerId": provider_id,
                "engine": "openai_compatible",
                "displayName": "Stark ACP Provider No Auth",
                "apiUrl": "https://stark.example/openai",
                "apiKey": "",
                "models": ["stark-3"],
                "supportsStreaming": false,
                "headers": {},
                "requiresAuth": false,
                "catalogProviderId": "zai",
                "toolshim": false
            }),
        )
        .await
        .expect("custom provider auth disable should succeed");
        assert_eq!(
            auth_disabled.get("status"),
            Some(&serde_json::json!({
                "providerId": provider_id,
                "isConfigured": true,
            })),
            "auth disable should invalidate the secret cache before status checks"
        );
        let no_auth_provider: DeclarativeProviderConfig =
            serde_json::from_str(&std::fs::read_to_string(&custom_provider_path).unwrap())
                .expect("no-auth provider should remain core-compatible");
        assert!(!no_auth_provider.requires_auth);
        assert_eq!(no_auth_provider.api_key_env, "");
        assert!(!no_auth_provider.toolshim);
        assert!(!no_auth_provider.preserves_thinking);
        assert!(
            matches!(
                Config::global().get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY"),
                Err(ConfigError::NotFound(_))
            ),
            "disabling auth should delete the previously stored API key"
        );

        let auth_reenabled_without_key = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/update",
            serde_json::json!({
                "providerId": provider_id,
                "engine": "openai_compatible",
                "displayName": "Stark ACP Provider Reauth",
                "apiUrl": "https://stark.example/openai",
                "apiKey": "",
                "models": ["stark-3"],
                "supportsStreaming": false,
                "headers": {},
                "requiresAuth": true,
                "catalogProviderId": "zai",
                "toolshim": false
            }),
        )
        .await
        .expect_err("re-enabling auth without a stored secret should fail");
        assert!(
            auth_reenabled_without_key
                .to_string()
                .contains("apiKey is required"),
            "unexpected error: {auth_reenabled_without_key}"
        );
        assert!(
            matches!(
                Config::global().get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY"),
                Err(ConfigError::NotFound(_))
            ),
            "blank re-enable should not recreate the previous API key"
        );

        let deleted = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/delete",
            serde_json::json!({ "providerId": provider_id }),
        )
        .await
        .expect("custom provider delete should succeed");
        assert_eq!(
            deleted.pointer("/providerId"),
            Some(&serde_json::json!(provider_id))
        );
        assert_eq!(
            deleted.get("refresh"),
            Some(&serde_json::json!({
                "started": [],
                "skipped": [],
            }))
        );
        assert!(
            !custom_provider_path.exists(),
            "custom provider delete should remove the declarative provider file"
        );
        assert!(
            matches!(
                Config::global().get_secret::<String>("CUSTOM_STARK_ACP_PROVIDER_API_KEY"),
                Err(ConfigError::NotFound(_))
            ),
            "custom provider delete should invalidate the secret cache before later reads"
        );

        let deleted_status = send_custom(
            conn.cx(),
            "_goose/unstable/providers/config/status",
            serde_json::json!({ "providerIds": [provider_id] }),
        )
        .await
        .expect("provider config status should succeed after delete");
        assert_eq!(
            deleted_status.pointer("/statuses/0"),
            Some(&serde_json::json!({
                "providerId": provider_id,
                "isConfigured": false,
            }))
        );

        for invalid_id in [
            "../escape",
            "foo/bar",
            ".hidden",
            "-bad",
            "",
            "Uppercase",
            "has space",
        ] {
            let read = send_custom(
                conn.cx(),
                "_goose/unstable/providers/custom/read",
                serde_json::json!({ "providerId": invalid_id }),
            )
            .await;
            assert!(
                read.is_err(),
                "invalid provider id should fail: {invalid_id:?}"
            );
        }

        for valid_id in ["custom_openai", "openai-compat", "a1"] {
            assert!(
                goose::config::declarative_providers::validate_provider_id(valid_id).is_ok(),
                "provider id should be valid: {valid_id}"
            );
        }

        for (name, patch) in [
            (
                "ftp URL",
                serde_json::json!({ "apiUrl": "ftp://example.com" }),
            ),
            ("relative URL", serde_json::json!({ "apiUrl": "/v1" })),
            ("empty models", serde_json::json!({ "models": [] })),
            ("blank models", serde_json::json!({ "models": [" ", "\n"] })),
            (
                "invalid header name",
                serde_json::json!({ "headers": { "Bad Header": "value" } }),
            ),
            (
                "invalid header value",
                serde_json::json!({ "headers": { "X-Test": "bad\r\nvalue" } }),
            ),
            (
                "unsupported engine",
                serde_json::json!({ "engine": "future_engine" }),
            ),
        ] {
            let mut payload = serde_json::json!({
                "engine": "openai_compatible",
                "displayName": format!("Invalid {name}"),
                "apiUrl": "https://api.example.test/v1",
                "apiKey": "secret",
                "models": ["model-a"],
                "headers": {},
                "requiresAuth": true,
                "toolshim": false
            });
            let payload_obj = payload.as_object_mut().unwrap();
            for (key, value) in patch.as_object().unwrap() {
                payload_obj.insert(key.clone(), value.clone());
            }

            let result = send_custom(
                conn.cx(),
                "_goose/unstable/providers/custom/create",
                payload,
            )
            .await;
            assert!(result.is_err(), "{name} should be rejected");
        }

        Config::global()
            .set_secret("SHARED_API_KEY", &"shared-secret")
            .unwrap();

        let shared = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/create",
            serde_json::json!({
                "engine": "openai_compatible",
                "displayName": "Shared Secret Test",
                "apiUrl": "https://api.example.test/v1",
                "apiKey": "owned-secret",
                "models": ["model-a"],
                "headers": {},
                "requiresAuth": true,
                "toolshim": false
            }),
        )
        .await
        .expect("shared-secret provider create should succeed");
        let shared_id = shared
            .get("providerId")
            .and_then(|provider_id| provider_id.as_str())
            .unwrap()
            .to_string();
        let shared_path = Paths::config_dir()
            .join("custom_providers")
            .join(format!("{shared_id}.json"));
        let mut shared_config: DeclarativeProviderConfig =
            serde_json::from_str(&std::fs::read_to_string(&shared_path).unwrap()).unwrap();
        shared_config.api_key_env = "SHARED_API_KEY".to_string();
        std::fs::write(
            &shared_path,
            serde_json::to_string_pretty(&shared_config).unwrap(),
        )
        .unwrap();
        Config::global().invalidate_secrets_cache();

        send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/update",
            serde_json::json!({
                "providerId": shared_id,
                "engine": "openai_compatible",
                "displayName": "Shared Secret Test",
                "apiUrl": "https://api.example.test/v1",
                "models": ["model-a"],
                "headers": {},
                "requiresAuth": false,
                "toolshim": false
            }),
        )
        .await
        .expect("disabling auth should preserve shared secrets");
        assert_eq!(
            Config::global()
                .get_secret::<String>("SHARED_API_KEY")
                .unwrap(),
            "shared-secret"
        );

        let shared_delete = send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/create",
            serde_json::json!({
                "engine": "openai_compatible",
                "displayName": "Shared Secret Delete",
                "apiUrl": "https://api.example.test/v1",
                "apiKey": "owned-secret",
                "models": ["model-a"],
                "headers": {},
                "requiresAuth": true,
                "toolshim": false
            }),
        )
        .await
        .expect("shared-delete provider create should succeed");
        let shared_delete_id = shared_delete
            .get("providerId")
            .and_then(|provider_id| provider_id.as_str())
            .unwrap()
            .to_string();
        let shared_delete_path = Paths::config_dir()
            .join("custom_providers")
            .join(format!("{shared_delete_id}.json"));
        let mut shared_delete_config: DeclarativeProviderConfig =
            serde_json::from_str(&std::fs::read_to_string(&shared_delete_path).unwrap()).unwrap();
        shared_delete_config.api_key_env = "SHARED_API_KEY".to_string();
        std::fs::write(
            &shared_delete_path,
            serde_json::to_string_pretty(&shared_delete_config).unwrap(),
        )
        .unwrap();
        Config::global().invalidate_secrets_cache();

        send_custom(
            conn.cx(),
            "_goose/unstable/providers/custom/delete",
            serde_json::json!({ "providerId": shared_delete_id }),
        )
        .await
        .expect("deleting provider should preserve shared secrets");
        assert_eq!(
            Config::global()
                .get_secret::<String>("SHARED_API_KEY")
                .unwrap(),
            "shared-secret"
        );
    });
}
