#![recursion_limit = "256"]
#[allow(dead_code)]
#[path = "acp_common_tests/mod.rs"]
mod common_tests;

use agent_client_protocol::schema::v1::{
    ContentBlock, McpServer, McpServerHttp, PromptRequest, SessionUpdate, StopReason, TextContent,
};
use common_tests::fixtures::server::AcpServerConnection;
use common_tests::fixtures::{
    run_test, send_custom, Connection, PermissionDecision, Session, SessionData,
    TestConnectionConfig,
};
use goose::acp::server::AcpProviderFactory;
use goose::providers::base::{MessageStream, Provider};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use goose_test_support::{EnforceSessionId, IgnoreSessionId, McpFixture, FAKE_CODE};
use serial_test::serial;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use common_tests::fixtures::OpenAiFixture;

const DEFAULT_ACP_TEST_CONFIG: &str =
    "GOOSE_MODEL: gpt-4o\nGOOSE_PROVIDER: openai\nGOOSE_DISABLE_KEYRING: true\n";

static ACP_CONFIG_ROOT: LazyLock<tempfile::TempDir> =
    LazyLock::new(|| tempfile::tempdir().unwrap());

fn write_acp_global_config(contents: &str) -> PathBuf {
    std::env::set_var("GOOSE_PATH_ROOT", ACP_CONFIG_ROOT.path());
    std::env::set_var("GOOSE_DISABLE_KEYRING", "1");
    let config_dir = goose::config::paths::Paths::config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut contents = contents.to_string();
    if !contents.contains("GOOSE_DISABLE_KEYRING") {
        contents.push_str("GOOSE_DISABLE_KEYRING: true\n");
    }
    std::fs::write(
        config_dir.join(goose::config::base::CONFIG_YAML_NAME),
        contents,
    )
    .unwrap();
    config_dir
}

struct MockProvider {
    name: String,
    recommended_models: Vec<String>,
    supported_models: Result<Vec<String>, ProviderError>,
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
        Ok(self.recommended_models.clone())
    }

    async fn fetch_supported_models(&self) -> Result<Vec<String>, ProviderError> {
        self.supported_models.clone()
    }
}

fn active_run_id_from_update(update: &SessionUpdate) -> Option<String> {
    let SessionUpdate::SessionInfoUpdate(info) = update else {
        return None;
    };
    info.meta
        .as_ref()?
        .get("goose")?
        .get("activeRunId")?
        .as_str()
        .map(ToString::to_string)
}

fn queued_steer_message_ids(updates: &[SessionUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| {
            let SessionUpdate::SessionInfoUpdate(info) = update else {
                return None;
            };
            info.meta
                .as_ref()?
                .get("goose")?
                .get("queuedSteer")?
                .get("messageId")?
                .as_str()
                .map(ToString::to_string)
        })
        .collect()
}

fn steer_chunk_message_ids(updates: &[SessionUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| {
            let SessionUpdate::UserMessageChunk(chunk) = update else {
                return None;
            };
            let goose = chunk.meta.as_ref()?.get("goose")?;
            goose.get("steer")?.as_bool().filter(|b| *b)?;
            goose.get("messageId")?.as_str().map(ToString::to_string)
        })
        .collect()
}

fn steer_chunk_texts(updates: &[SessionUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| {
            // A steered message is a user message injected mid-run, so it must
            // arrive as a UserMessageChunk (matching the replay path), never an
            // AgentMessageChunk.
            let SessionUpdate::UserMessageChunk(chunk) = update else {
                return None;
            };
            let ContentBlock::Text(text) = &chunk.content else {
                return None;
            };
            let is_steer = chunk
                .meta
                .as_ref()
                .and_then(|m| m.get("goose"))
                .and_then(|g| g.get("steer"))
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            is_steer.then(|| text.text.clone())
        })
        .collect()
}

fn collect_agent_text(updates: &[SessionUpdate]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
#[serial]
fn test_custom_get_tools() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let mut conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let SessionData { session, .. } = conn.new_session().await.unwrap();
        let session_id = session.session_id().0.clone();

        let result = send_custom(
            conn.cx(),
            "_goose/unstable/tools/list",
            serde_json::json!({ "sessionId": session_id }),
        )
        .await;
        assert!(result.is_ok(), "expected ok, got: {:?}", result);

        let response = result.unwrap();
        let tools = response.get("tools").expect("missing 'tools' field");
        assert!(tools.is_array(), "tools should be array");
    });
}

#[test]
#[serial]
fn test_custom_get_extensions() {
    let config_key = "test-stdio-acp-mutation-flow";
    let _guard = env_lock::lock_env([("EXTENSIONS", None::<&str>)]);
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let add_result = send_custom(
            conn.cx(),
            "_goose/unstable/config/extensions/add",
            serde_json::json!({
                "enabled": true,
                "extension": {
                    "type": "mcp",
                    "description": "Test stdio",
                    "envKeys": ["SECRET_TOKEN"],
                    "timeout": 42,
                    "server": {
                        "type": "stdio",
                        "name": config_key,
                        "command": "test-command",
                        "args": ["--flag", "value"],
                        "env": [
                            { "name": "INLINE_TOKEN", "value": "inline-secret" }
                        ]
                    }
                }
            }),
        )
        .await;
        assert!(add_result.is_ok(), "expected ok, got: {:?}", add_result);
        let stored_inline_token = goose::config::Config::global()
            .get_secret::<String>("INLINE_TOKEN")
            .expect("inline env should be saved as a secret");
        assert!(
            stored_inline_token == "inline-secret",
            "inline env secret was not saved correctly"
        );

        let list_extension = || async {
            let result = send_custom(
                conn.cx(),
                "_goose/unstable/config/extensions/list",
                serde_json::json!({}),
            )
            .await;
            assert!(result.is_ok(), "expected ok, got: {:?}", result);

            let response = result.unwrap();
            let extensions = response
                .get("extensions")
                .and_then(|extensions| extensions.as_array())
                .expect("extensions should be an array");
            extensions
                .iter()
                .find(|entry| entry["configKey"] == config_key)
                .cloned()
        };

        let entry = list_extension()
            .await
            .unwrap_or_else(|| panic!("missing added extension entry"));
        assert_eq!(entry["enabled"], true);
        assert_eq!(entry["configKey"], config_key);

        let extension = &entry["extension"];
        assert_eq!(extension["type"], "mcp");
        assert_eq!(
            extension["envKeys"],
            serde_json::json!(["SECRET_TOKEN", "INLINE_TOKEN"])
        );
        assert_eq!(extension["description"], "Test stdio");
        assert_eq!(extension["timeout"], 42);
        assert!(extension.get("socket").is_none());

        let server = &extension["server"];
        assert_eq!(server["name"], config_key);
        assert_eq!(server["command"], "test-command");
        assert_eq!(server["args"], serde_json::json!(["--flag", "value"]));
        assert_eq!(server["env"], serde_json::json!([]));

        let set_enabled_result = send_custom(
            conn.cx(),
            "_goose/unstable/config/extensions/set-enabled",
            serde_json::json!({
                "configKey": config_key,
                "enabled": false,
            }),
        )
        .await;
        assert!(
            set_enabled_result.is_ok(),
            "expected ok, got: {:?}",
            set_enabled_result
        );

        let entry = list_extension()
            .await
            .unwrap_or_else(|| panic!("missing disabled extension entry"));
        assert_eq!(entry["enabled"], false);

        let remove_result = send_custom(
            conn.cx(),
            "_goose/unstable/config/extensions/remove",
            serde_json::json!({
                "configKey": config_key,
            }),
        )
        .await;
        assert!(
            remove_result.is_ok(),
            "expected ok, got: {:?}",
            remove_result
        );

        assert!(
            list_extension().await.is_none(),
            "removed extension should not be listed"
        );
    });
}

#[test]
#[serial]
fn test_custom_session_extensions_add_list_remove() {
    let extension_name = "summarize";
    let _guard = env_lock::lock_env([("EXTENSIONS", None::<&str>)]);
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let mut conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let SessionData { session, .. } = conn.new_session().await.unwrap();
        let session_id = session.session_id().0.clone();

        let list_extension = || async {
            let result = send_custom(
                conn.cx(),
                "_goose/unstable/session/extensions/list",
                serde_json::json!({ "sessionId": session_id.clone() }),
            )
            .await;
            assert!(result.is_ok(), "expected ok, got: {:?}", result);

            let response = result.unwrap();
            let extensions = response
                .get("extensions")
                .and_then(|extensions| extensions.as_array())
                .expect("extensions should be an array");
            extensions
                .iter()
                .find(|entry| entry["extension"]["name"] == extension_name)
                .cloned()
        };

        assert!(
            list_extension().await.is_none(),
            "{extension_name} should not be enabled before add"
        );

        let add_result = send_custom(
            conn.cx(),
            "_goose/unstable/session/extensions/add",
            serde_json::json!({
                "sessionId": session_id.clone(),
                "extension": {
                    "type": "platform",
                    "name": extension_name,
                    "description": "Load files/directories and get an LLM summary in a single call",
                    "displayName": "Summarize",
                    "bundled": true
                }
            }),
        )
        .await;
        assert!(add_result.is_ok(), "expected ok, got: {:?}", add_result);

        let entry = list_extension()
            .await
            .unwrap_or_else(|| panic!("missing added session extension"));
        assert_eq!(entry["extensionKey"], extension_name);
        let extension = &entry["extension"];
        assert_eq!(extension["type"], "platform");
        assert_eq!(extension["name"], extension_name);

        let unknown_remove_result = send_custom(
            conn.cx(),
            "_goose/unstable/session/extensions/remove",
            serde_json::json!({
                "sessionId": session_id.clone(),
                "extensionKey": "missing-extension",
            }),
        )
        .await;
        assert_eq!(
            unknown_remove_result.unwrap_err(),
            agent_client_protocol::Error::invalid_params()
                .data("Extension 'missing-extension' not found")
        );
        assert!(
            list_extension().await.is_some(),
            "unknown key must not remove another extension"
        );

        let remove_result = send_custom(
            conn.cx(),
            "_goose/unstable/session/extensions/remove",
            serde_json::json!({
                "sessionId": session_id.clone(),
                "extensionKey": extension_name,
            }),
        )
        .await;
        assert!(
            remove_result.is_ok(),
            "expected ok, got: {:?}",
            remove_result
        );

        assert!(
            list_extension().await.is_none(),
            "removed session extension should not be listed"
        );
    });
}

#[test]
#[serial]
fn test_custom_prompt_methods() {
    let _guard = env_lock::lock_env([("EXTENSIONS", None::<&str>)]);
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let list_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/list",
            serde_json::json!({}),
        )
        .await
        .expect("list prompts should succeed");
        let prompts = list_response["prompts"]
            .as_array()
            .expect("prompts should be an array");
        assert!(
            prompts.iter().any(|prompt| prompt["name"] == "system.md"),
            "system.md should be listed"
        );

        let get_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/get",
            serde_json::json!({ "name": "system.md" }),
        )
        .await
        .expect("get prompt should succeed");
        assert_eq!(get_response["name"], "system.md");
        assert!(get_response["content"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert_eq!(get_response["isCustomized"], false);

        let content = "custom acp system prompt";
        let save_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/save",
            serde_json::json!({ "name": "system.md", "content": content }),
        )
        .await
        .expect("save prompt should succeed");
        assert_eq!(save_response["message"], "Saved prompt: system.md");

        let get_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/get",
            serde_json::json!({ "name": "system.md" }),
        )
        .await
        .expect("get saved prompt should succeed");
        assert_eq!(get_response["content"], content);
        assert_eq!(get_response["isCustomized"], true);

        let reset_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/reset",
            serde_json::json!({ "name": "system.md" }),
        )
        .await
        .expect("reset prompt should succeed");
        assert_eq!(
            reset_response["message"],
            "Reset prompt to default: system.md"
        );

        let get_response = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/get",
            serde_json::json!({ "name": "system.md" }),
        )
        .await
        .expect("get reset prompt should succeed");
        assert_eq!(get_response["isCustomized"], false);
        assert_ne!(get_response["content"], content);

        let missing = send_custom(
            conn.cx(),
            "_goose/unstable/config/prompts/get",
            serde_json::json!({ "name": "missing.md" }),
        )
        .await
        .expect_err("unknown prompt should fail");
        assert_eq!(
            missing.code,
            agent_client_protocol::ErrorCode::InvalidParams
        );
    });
}

#[test]
#[serial]
fn test_steer_session_adds_input_to_active_prompt() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        // Two-turn exchange: the first turn ends the turn with plain text. A
        // steer queued before the turn ends keeps the loop alive (it flips
        // `exit_chat` back to false), so a second provider request fires whose
        // body must now contain the steered text.
        let openai = OpenAiFixture::new(
            vec![
                (
                    "start work".to_string(),
                    include_str!("acp_test_data/openai_steer_first.txt"),
                ),
                (
                    "steer while active".to_string(),
                    include_str!("acp_test_data/openai_steer_second.txt"),
                ),
            ],
            Arc::new(IgnoreSessionId),
        )
        .await;
        let mut conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let SessionData { session, .. } = conn.new_session().await.unwrap();
        let session_id = session.session_id().0.to_string();
        let acp_session_id = session.session_id().clone();

        let mut prompt = Box::pin(
            conn.cx()
                .send_request(PromptRequest::new(
                    acp_session_id,
                    vec![ContentBlock::Text(TextContent::new("start work"))],
                ))
                .block_task(),
        );
        let mut steer_sent = false;
        let mut steer_message_id: Option<String> = None;
        let mut final_response = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);

        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                response = &mut prompt => {
                    final_response = Some(response.unwrap());
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(10)), if !steer_sent => {
                    let updates = session.session_updates();
                    if let Some(run_id) = updates.iter().find_map(active_run_id_from_update) {
                        let response = send_custom(
                            conn.cx(),
                            "_goose/unstable/session/steer",
                            serde_json::json!({
                                "sessionId": session_id,
                                "expectedRunId": run_id,
                                "prompt": [
                                    { "type": "text", "text": "steer while active" }
                                ]
                            }),
                        )
                        .await
                        .unwrap();
                        assert_eq!(response["runId"], run_id);
                        let mid = response["messageId"].as_str();
                        assert!(
                            mid.is_some_and(|id| !id.is_empty()),
                            "steer response must return a messageId for correlation, got: {response:?}"
                        );
                        steer_message_id = mid.map(ToString::to_string);
                        steer_sent = true;
                    }
                }
            }
        }

        let response = final_response.expect("prompt did not complete");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert!(steer_sent, "test never observed an active run id");

        let updates = session.session_updates();
        let agent_text = collect_agent_text(&updates);
        assert!(
            agent_text.contains("saw steer"),
            "expected provider to receive steered input, got: {agent_text:?}"
        );

        // The echoed steer prompt must be marked structurally so the client
        // can locate the boundary without matching user-visible text.
        let steer_chunks = steer_chunk_texts(&updates);
        assert!(
            steer_chunks
                .iter()
                .any(|t| t.contains("steer while active")),
            "expected a chunk marked _meta.goose.steer with the steer text, got: {steer_chunks:?}"
        );

        // The queued steer must be announced (so a UI can show it as pending)
        // and carry the same messageId returned by the steer response and later
        // stamped on the picked-up UserMessageChunk.
        let steer_message_id = steer_message_id.expect("steer response had no messageId");
        let queued_ids = queued_steer_message_ids(&updates);
        assert!(
            queued_ids.contains(&steer_message_id),
            "expected a queuedSteer SessionInfoUpdate with messageId {steer_message_id:?}, got: {queued_ids:?}"
        );
        let picked_up_ids = steer_chunk_message_ids(&updates);
        assert!(
            picked_up_ids.contains(&steer_message_id),
            "picked-up steer chunk must carry the queued messageId {steer_message_id:?} for correlation, got: {picked_up_ids:?}"
        );
    });
}

#[test]
#[serial]
fn test_custom_list_builtin_skill_sources() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/sources/list",
            serde_json::json!({ "type": "builtinSkill" }),
        )
        .await
        .expect("builtin skill sources list should succeed");
        let sources = response
            .get("sources")
            .and_then(|value| value.as_array())
            .expect("missing sources array");
        let builtin = sources
            .iter()
            .find(|source| source.get("name") == Some(&serde_json::json!("goose-doc-guide")))
            .expect("expected goose-doc-guide builtin skill");

        assert_eq!(
            builtin.get("type"),
            Some(&serde_json::json!("builtinSkill"))
        );
        assert_eq!(builtin.get("global"), Some(&serde_json::json!(true)));
        assert_eq!(
            builtin.get("path"),
            Some(&serde_json::json!("builtin://skills/goose-doc-guide"))
        );
    });
}

#[test]
#[serial]
fn test_custom_provider_inventory_includes_metadata() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/providers/list",
            serde_json::json!({}),
        )
        .await
        .expect("provider inventory should succeed");
        let providers = response
            .get("entries")
            .and_then(|value| value.as_array())
            .expect("missing entries array");
        let openai = providers
            .iter()
            .find(|provider| provider.get("providerId") == Some(&serde_json::json!("openai")))
            .expect("expected openai inventory entry");

        assert!(openai.get("providerName").is_some(), "missing providerName");
        assert!(openai.get("description").is_some(), "missing description");
        assert!(openai.get("defaultModel").is_some(), "missing defaultModel");
        assert!(openai.get("providerType").is_some(), "missing providerType");
        assert!(openai.get("configKeys").is_some(), "missing configKeys");
        assert!(openai.get("setupSteps").is_some(), "missing setupSteps");
    });
}

#[test]
#[serial]
fn test_custom_preferences_read_save() {
    let config_dir = write_acp_global_config(
        "GOOSE_MODEL: gpt-4o\nGOOSE_PROVIDER: openai\nGOOSE_AUTO_COMPACT_THRESHOLD: 0.7\nGOOSE_THINKING_EFFORT: high\nVOICE_AUTO_SUBMIT_PHRASES: send it\n",
    );

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let config = TestConnectionConfig {
            data_root: config_dir,
            ..Default::default()
        };
        let conn = AcpServerConnection::new(config, openai).await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/preferences/read",
            serde_json::json!({
                "keys": [
                    "autoCompactThreshold",
                    "gooseThinkingEffort",
                    "voiceAutoSubmitPhrases",
                    "voiceDictationPreferredMic"
                ],
            }),
        )
        .await
        .expect("preferences read should succeed");
        assert_eq!(
            response.get("values"),
            Some(&serde_json::json!([
                { "key": "autoCompactThreshold", "value": 0.7 },
                { "key": "gooseThinkingEffort", "value": "high" },
                { "key": "voiceAutoSubmitPhrases", "value": "send it" },
                { "key": "voiceDictationPreferredMic", "value": null },
            ]))
        );

        send_custom(
            conn.cx(),
            "_goose/unstable/preferences/save",
            serde_json::json!({
                "values": [
                    { "key": "gooseThinkingEffort", "value": "disabled" },
                    { "key": "voiceDictationProvider", "value": "__disabled__" },
                    { "key": "voiceDictationPreferredMic", "value": "mic-1" }
                ],
            }),
        )
        .await
        .expect("preferences save should succeed");

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/preferences/read",
            serde_json::json!({
                "keys": ["gooseThinkingEffort", "voiceDictationProvider", "voiceDictationPreferredMic"],
            }),
        )
        .await
        .expect("preferences read after save should succeed");
        assert_eq!(
            response.get("values"),
            Some(&serde_json::json!([
                { "key": "gooseThinkingEffort", "value": "off" },
                { "key": "voiceDictationProvider", "value": "__disabled__" },
                { "key": "voiceDictationPreferredMic", "value": "mic-1" },
            ]))
        );
    });
}

#[test]
#[serial]
fn test_custom_preferences_save_rejects_invalid_values() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let invalid_payloads = [
            serde_json::json!({
                "values": [{ "key": "autoCompactThreshold", "value": 0 }],
            }),
            serde_json::json!({
                "values": [{ "key": "autoCompactThreshold", "value": 1.1 }],
            }),
            serde_json::json!({
                "values": [{ "key": "gooseThinkingEffort", "value": "bogus" }],
            }),
            serde_json::json!({
                "values": [{ "key": "gooseThinkingEffort", "value": ["high"] }],
            }),
            serde_json::json!({
                "values": [{ "key": "voiceAutoSubmitPhrases", "value": ["send"] }],
            }),
            serde_json::json!({
                "values": [{ "key": "voiceDictationProvider", "value": "bogus" }],
            }),
            serde_json::json!({
                "values": [{ "key": "voiceDictationPreferredMic", "value": "" }],
            }),
        ];

        for payload in invalid_payloads {
            let result = send_custom(conn.cx(), "_goose/unstable/preferences/save", payload).await;
            assert!(result.is_err(), "expected invalid params error");
        }

        let result = send_custom(
            conn.cx(),
            "_goose/unstable/preferences/save",
            serde_json::json!({
                "values": [
                    { "key": "voiceDictationPreferredMic", "value": "mic-1" },
                    { "key": "voiceDictationProvider", "value": "bogus" }
                ],
            }),
        )
        .await;
        assert!(result.is_err(), "expected invalid params error");

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/preferences/read",
            serde_json::json!({
                "keys": ["voiceDictationPreferredMic"],
            }),
        )
        .await
        .expect("preferences read should succeed");
        assert_eq!(
            response.get("values"),
            Some(&serde_json::json!([
                { "key": "voiceDictationPreferredMic", "value": null },
            ]))
        );
    });
}

#[test]
#[serial]
fn test_custom_defaults_read() {
    let config_dir = write_acp_global_config(
        "GOOSE_MODEL: claude-3-5-haiku-latest\nGOOSE_PROVIDER: anthropic\n",
    );

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let config = TestConnectionConfig {
            data_root: config_dir,
            ..Default::default()
        };
        let conn = AcpServerConnection::new(config, openai).await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/defaults/read",
            serde_json::json!({}),
        )
        .await
        .expect("defaults read should succeed");
        assert_eq!(
            response,
            serde_json::json!({
                "providerId": "anthropic",
                "modelId": "claude-3-5-haiku-latest",
            })
        );
    });
}

// Regression test for #10401: a custom model that is not present in a provider's
// (non-exhaustive) inventory must still be accepted when saved as the default from
// a brand-new chat, matching the CLI and in-session model-selection paths where
// custom/unlisted model entry is always allowed (#7255).
#[test]
#[serial]
fn test_custom_defaults_save_allows_unlisted_model() {
    let _env = env_lock::lock_env([("ANTHROPIC_API_KEY", Some("test-key"))]);
    let config_dir = write_acp_global_config(
        "GOOSE_MODEL: claude-3-5-haiku-latest\nGOOSE_PROVIDER: anthropic\n",
    );

    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let config = TestConnectionConfig {
            data_root: config_dir,
            ..Default::default()
        };
        let conn = AcpServerConnection::new(config, openai).await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/defaults/save",
            serde_json::json!({
                "providerId": "anthropic",
                "modelId": "custom-unlisted-model",
            }),
        )
        .await
        .expect("saving an unlisted custom model as the default should succeed");

        assert_eq!(
            response,
            serde_json::json!({
                "providerId": "anthropic",
                "modelId": "custom-unlisted-model",
            })
        );
    });
}

#[test]
#[serial]
fn test_raw_config_and_secret_methods_are_removed() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        for method in [
            "_goose/config/read",
            "_goose/config/upsert",
            "_goose/config/remove",
            "_goose/secret/check",
            "_goose/secret/upsert",
            "_goose/secret/remove",
        ] {
            let result = send_custom(conn.cx(), method, serde_json::json!({})).await;
            assert!(result.is_err(), "{method} should be removed");
        }
    });
}

#[test]
#[serial]
fn test_provider_switching_updates_session_state() {
    let _env = env_lock::lock_env([("ANTHROPIC_API_KEY", Some("test-key"))]);
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let config = TestConnectionConfig {
            current_model: "gpt-4o".to_string(),
            ..Default::default()
        };
        let mut conn = AcpServerConnection::new(config, openai).await;

        let SessionData { session, .. } = conn.new_session().await.unwrap();
        let session_id = session.session_id().0.clone();

        conn.set_config_option(&session_id, "provider", "anthropic")
            .await
            .expect("provider switch to anthropic should succeed");

        conn.set_config_option(&session_id, "provider", "openai")
            .await
            .expect("provider switch to openai should succeed");

        conn.set_config_option(&session_id, "provider", "goose")
            .await
            .expect("provider reset to goose should succeed");
    });
}

#[test]
#[serial]
fn test_custom_unknown_method() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let conn = AcpServerConnection::new(TestConnectionConfig::default(), openai).await;

        let result = send_custom(conn.cx(), "_unknown/method", serde_json::json!({})).await;
        assert!(result.is_err(), "expected method_not_found error");
    });
}

#[test]
#[serial]
fn test_developer_fs_requests_use_acp_session_id() {
    run_test(async {
        let seen_session_id = Arc::new(Mutex::new(None::<String>));
        let seen_session_id_clone = Arc::clone(&seen_session_id);
        let openai = OpenAiFixture::new(
            vec![
                (
                    "Use the read tool to read /tmp/test_acp_read.txt and output only its contents."
                        .to_string(),
                    include_str!("acp_test_data/openai_fs_read_tool_call.txt"),
                ),
                (
                    r#""content":"test-read-content-12345""#.into(),
                    include_str!("acp_test_data/openai_fs_read_tool_result.txt"),
                ),
            ],
            Arc::new(IgnoreSessionId),
        )
        .await;
        let config_dir = write_acp_global_config(&format!(
            "GOOSE_MODEL: gpt-4.1\nGOOSE_PROVIDER: openai\nOPENAI_HOST: {}\n",
            openai.uri()
        ));
        let config = TestConnectionConfig {
            // gpt-5-nano routes to the Responses API; use a Chat Completions
            // model so the canned SSE fixtures are parsed correctly.
            data_root: config_dir,
            current_model: "gpt-4.1".to_string(),
            read_text_file: Some(Arc::new(move |req| {
                *seen_session_id_clone.lock().unwrap() = Some(req.session_id.0.to_string());
                Ok(
                    agent_client_protocol::schema::v1::ReadTextFileResponse::new(
                        "test-read-content-12345",
                    ),
                )
            })),
            ..Default::default()
        };
        let mut conn = AcpServerConnection::new(config, openai).await;

        let SessionData { mut session, .. } = conn.new_session().await.unwrap();
        let acp_session_id = session.session_id().0.to_string();

        let output = session
            .prompt(
                "Use the read tool to read /tmp/test_acp_read.txt and output only its contents.",
                PermissionDecision::Cancel,
            )
            .await
            .expect("prompt should succeed");

        assert_eq!(output.text, "test-read-content-12345");
        assert_eq!(
            seen_session_id.lock().unwrap().as_deref(),
            Some(acp_session_id.as_str()),
            "ACP read request should use the ACP session/thread ID",
        );
    });
}

#[test]
#[serial]
fn test_custom_provider_supported_models_lists_raw_provider_models() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let provider_factory: AcpProviderFactory = Arc::new(
            |provider_name, _extensions, _working_dir, use_default_model| {
                assert!(use_default_model);
                Box::pin(async move {
                    Ok(Arc::new(MockProvider {
                        name: provider_name,
                        recommended_models: vec!["canonical-filtered-model".to_string()],
                        supported_models: Ok(vec![
                            "goose-claude-opus-4-8".to_string(),
                            "raw-databricks-endpoint".to_string(),
                        ]),
                    }) as Arc<dyn Provider>)
                })
            },
        );
        let conn = AcpServerConnection::new(
            TestConnectionConfig {
                provider_factory: Some(provider_factory),
                ..Default::default()
            },
            openai,
        )
        .await;

        let response = send_custom(
            conn.cx(),
            "_goose/unstable/providers/supported-models/list",
            serde_json::json!({ "providerId": "openai" }),
        )
        .await
        .expect("provider supported models list should succeed");

        assert_eq!(
            response.get("providerId"),
            Some(&serde_json::json!("openai"))
        );
        assert_eq!(
            response.get("models"),
            Some(&serde_json::json!([
                "goose-claude-opus-4-8",
                "raw-databricks-endpoint"
            ]))
        );
    });
}

#[test]
#[serial]
fn test_custom_provider_supported_models_maps_not_configured_error() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let provider_factory: AcpProviderFactory = Arc::new(|provider_name, _, _, _| {
            Box::pin(async move {
                Ok(Arc::new(MockProvider {
                    name: provider_name,
                    recommended_models: Vec::new(),
                    supported_models: Err(ProviderError::NotConfigured),
                }) as Arc<dyn Provider>)
            })
        });
        let conn = AcpServerConnection::new(
            TestConnectionConfig {
                provider_factory: Some(provider_factory),
                ..Default::default()
            },
            openai,
        )
        .await;

        let error = send_custom(
            conn.cx(),
            "_goose/unstable/providers/supported-models/list",
            serde_json::json!({ "providerId": "openai" }),
        )
        .await
        .expect_err("not configured should be returned to the client");

        assert_eq!(error.code, agent_client_protocol::ErrorCode::InvalidParams);
        assert!(error.to_string().contains("Provider is not configured"));
    });
}

#[test]
#[serial]
fn test_custom_provider_supported_models_maps_authentication_error() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let openai = OpenAiFixture::new(vec![], Arc::new(EnforceSessionId::default())).await;
        let provider_factory: AcpProviderFactory = Arc::new(|provider_name, _, _, _| {
            Box::pin(async move {
                Ok(Arc::new(MockProvider {
                    name: provider_name,
                    recommended_models: Vec::new(),
                    supported_models: Err(ProviderError::Authentication(
                        "credentials rejected".to_string(),
                    )),
                }) as Arc<dyn Provider>)
            })
        });
        let conn = AcpServerConnection::new(
            TestConnectionConfig {
                provider_factory: Some(provider_factory),
                ..Default::default()
            },
            openai,
        )
        .await;

        let error = send_custom(
            conn.cx(),
            "_goose/unstable/providers/supported-models/list",
            serde_json::json!({ "providerId": "openai" }),
        )
        .await
        .expect_err("authentication failure should be returned to the client");

        assert_eq!(error.code, agent_client_protocol::ErrorCode::AuthRequired);
        assert!(error.to_string().contains("credentials rejected"));
    });
}

const APP_TOOL: &str = "mcp-fixture__get_code";

async fn connect_with_mcp_fixture(mcp_url: &str, mode: &str) -> (AcpServerConnection, String) {
    let openai = OpenAiFixture::new(vec![], Arc::new(IgnoreSessionId)).await;
    let mcp_servers = vec![McpServer::Http(McpServerHttp::new("mcp-fixture", mcp_url))];
    let config = TestConnectionConfig {
        mcp_servers,
        ..Default::default()
    };
    let mut conn = AcpServerConnection::new(config, openai).await;
    let SessionData { session, .. } = conn.new_session().await.unwrap();
    let session_id = session.session_id().0.to_string();
    conn.set_mode(&session_id, mode).await.unwrap();
    (conn, session_id)
}

async fn call_app_tool(
    conn: &AcpServerConnection,
    session_id: &str,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    send_custom(
        conn.cx(),
        "_goose/unstable/tools/call",
        serde_json::json!({
            "sessionId": session_id,
            "extensionName": "mcp-fixture",
            "name": APP_TOOL
        }),
    )
    .await
}

#[test]
#[serial]
fn test_app_tool_call_requires_auto_mode() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let mcp = McpFixture::new().await;

        for mode in ["approve", "smart_approve", "chat"] {
            let (conn, session_id) = connect_with_mcp_fixture(&mcp.url, mode).await;
            let error = call_app_tool(&conn, &session_id)
                .await
                .expect_err("app tool call should be rejected outside auto mode");
            assert_eq!(error.code, agent_client_protocol::ErrorCode::InvalidParams);
            assert!(error
                .to_string()
                .contains("app tool calls require auto mode"));
        }
    });
}

#[test]
#[serial]
fn test_app_tool_call_dispatched_in_auto_mode() {
    write_acp_global_config(DEFAULT_ACP_TEST_CONFIG);
    run_test(async move {
        let mcp = McpFixture::new().await;
        let (conn, session_id) = connect_with_mcp_fixture(&mcp.url, "auto").await;

        let response = call_app_tool(&conn, &session_id)
            .await
            .expect("app tool call should dispatch in auto mode");
        let text = response["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
            .collect::<String>();
        assert!(text.contains(FAKE_CODE));
    });
}
