// Required when compiled as standalone test "common"; harmless warning when included as module.
#![recursion_limit = "256"]
#![allow(unused_attributes)]

#[path = "../acp_fixtures/mod.rs"]
pub mod fixtures;
use agent_client_protocol::schema::v1::{
    ContentBlock, ListSessionsResponse, McpServer, McpServerHttp, SessionInfo, SessionModeId,
    SessionUpdate, ToolCallStatus, ToolKind,
};
use fixtures::{
    assert_notifications, Connection, FsFixture, Notification, OpenAiFixture, PermissionDecision,
    Session, SessionData, TerminalCall, TerminalFixture, TestConnectionConfig,
};
use fs_err as fs;
use goose::acp::server::AcpProviderFactory;
use goose::config::base::CONFIG_YAML_NAME;
use goose::config::GooseMode;
use goose::session::{EnabledExtensionsState, SessionManager};
use goose_test_support::{McpFixture, FAKE_CODE, TEST_IMAGE_B64, TEST_MODEL};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use std::time::Duration;

const SHELL_TEST_CONTENT: &str = "test-shell-content-98765";
pub const TURN_CONTEXT_OPEN: &str = r#"\n<turn-context>"#;
/// Session name produced by `OPENAI_SESSION_NAME_RESPONSE`.
pub const GENERATED_SESSION_TITLE: &str = "Generated Test Title";
pub const OPENAI_SESSION_NAME_RESPONSE: &str = r#"data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1766229303,"model":"gpt-5-nano","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1766229303,"model":"gpt-5-nano","choices":[{"index":0,"delta":{"content":"Generated Test Title"},"finish_reason":null}]}

data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1766229303,"model":"gpt-5-nano","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1766229303,"model":"gpt-5-nano","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110}}

data: [DONE]"#;

struct BasicSession<C: Connection> {
    conn: C,
    session: C::Session,
}

async fn new_basic_session<C: Connection>(config: TestConnectionConfig) -> BasicSession<C> {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            format!("what is 1+1{TURN_CONTEXT_OPEN}"),
            include_str!("../acp_test_data/openai_basic.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");

    BasicSession { conn, session }
}

pub async fn run_list_sessions<C: Connection>() {
    let BasicSession { conn, session } =
        new_basic_session::<C>(TestConnectionConfig::default()).await;
    let mut response = conn.list_sessions().await.unwrap();
    for s in &mut response.sessions {
        s.updated_at = None;
        // createdAt is a dynamic timestamp — verify it exists then remove for comparison.
        if let Some(ref mut meta) = s.meta {
            assert!(meta.get("createdAt").and_then(|v| v.as_str()).is_some());
            assert!(meta.get("lastMessageAt").and_then(|v| v.as_str()).is_some());
            meta.remove("createdAt");
            meta.remove("lastMessageAt");
            // Provider/model metadata varies by test fixture; not relevant here.
            meta.remove("providerId");
            meta.remove("modelId");
        }
    }
    let mut expected_meta = serde_json::Map::new();
    expected_meta.insert(
        "messageCount".to_string(),
        serde_json::Value::Number(2.into()),
    );
    expected_meta.insert("userSetName".to_string(), serde_json::Value::Bool(false));
    expected_meta.insert(
        "sessionType".to_string(),
        serde_json::Value::String("acp".to_string()),
    );
    expected_meta.insert("hasRecipe".to_string(), serde_json::Value::Bool(false));
    assert_eq!(
        response,
        ListSessionsResponse::new(vec![SessionInfo::new(
            session.session_id().clone(),
            session.work_dir()
        )
        .title("New Chat".to_string())
        .meta(expected_meta)])
    );
}

pub async fn run_session_name_update_notification<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![
            (
                format!("what should we call this conversation?{TURN_CONTEXT_OPEN}"),
                include_str!("../acp_test_data/openai_basic.txt"),
            ),
            (
                "Generate a short title for the above messages.".into(),
                OPENAI_SESSION_NAME_RESPONSE,
            ),
        ],
        expected_session_id.clone(),
    )
    .await;
    let config = TestConnectionConfig {
        disable_session_naming: false,
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(
            "what should we call this conversation?",
            PermissionDecision::Cancel,
        )
        .await
        .unwrap();
    assert_eq!(output.text, "2");

    let mut notifications = session.notifications();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !notifications
        .iter()
        .any(|n| matches!(n, Notification::SessionInfoUpdate { .. }))
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
        notifications.extend(session.notifications());
    }

    let update = notifications
        .iter()
        .find_map(|notification| match notification {
            Notification::SessionInfoUpdate {
                title,
                updated_at,
                message_count,
                user_set_name,
            } => Some((title, updated_at, message_count, user_set_name)),
            _ => None,
        })
        .expect("expected generated session name notification");
    assert_eq!(update.0.as_deref(), Some(GENERATED_SESSION_TITLE));
    assert!(update.1.is_some());
    assert!(update.2.unwrap_or_default() >= 1);
    assert_eq!(*update.3, Some(false));
}

pub async fn run_close_session<C: Connection>() {
    let BasicSession { conn, session } =
        new_basic_session::<C>(TestConnectionConfig::default()).await;
    let sid = &session.session_id().0;
    let data_root = conn.data_root();

    conn.close_session(sid).await.unwrap();

    // Provider close drops the connection, so verify via DB not list_sessions.
    let db_path = data_root.join("sessions").join("sessions.db");
    let pool = SqlitePoolOptions::new()
        .connect(&format!("sqlite:{}?mode=ro", db_path.display()))
        .await
        .unwrap();
    let db_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(db_ids.len(), 1);

    let expected_session_id = C::expected_session_id();
    expected_session_id.set(sid);
    expected_session_id.assert_matches(&db_ids[0]);
}

pub async fn run_delete_session<C: Connection>() {
    let BasicSession { mut conn, session } =
        new_basic_session::<C>(TestConnectionConfig::default()).await;
    let sid = session.session_id().0.to_string();

    let before: Vec<_> = conn
        .list_sessions()
        .await
        .unwrap()
        .sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();
    assert!(before.contains(session.session_id()));

    conn.delete_session(&sid).await.unwrap();

    let after: Vec<_> = conn
        .list_sessions()
        .await
        .unwrap()
        .sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();
    assert!(!after.contains(session.session_id()));

    let err = conn.load_session(&sid, vec![]).await.unwrap_err();
    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(
        acp_err.code,
        agent_client_protocol::ErrorCode::ResourceNotFound
    );
}

pub async fn run_config_mcp<C: Connection>() {
    let temp_dir = tempfile::tempdir().unwrap();
    let expected_session_id = C::expected_session_id();
    let prompt = "Use the get_code tool and output only its result.";
    let mcp = McpFixture::new().await;

    let config_yaml = format!(
        "GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nextensions:\n  mcp-fixture:\n    enabled: true\n    type: streamable_http\n    name: mcp-fixture\n    description: MCP fixture\n    uri: \"{}\"\n",
        mcp.url
    );
    fs::write(temp_dir.path().join(CONFIG_YAML_NAME), config_yaml).unwrap();

    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, FAKE_CODE);
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

// Also proves developer loaded from config.yaml (not CLI args) gets ACP fs delegation.
pub async fn run_fs_read_text_file_true<C: Connection>() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_yaml = format!(
        "GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nextensions:\n  developer:\n    enabled: true\n    type: platform\n    name: developer\n    description: Developer\n    display_name: Developer\n    bundled: true\n    available_tools: []\n"
    );
    fs::write(temp_dir.path().join(CONFIG_YAML_NAME), config_yaml).unwrap();

    let expected_session_id = C::expected_session_id();
    let prompt = "Use the read tool to read /tmp/test_acp_read.txt and output only its contents.";
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_fs_read_tool_call.txt"),
            ),
            (
                r#""content":"test-read-content-12345""#.into(),
                include_str!("../acp_test_data/openai_fs_read_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let fs = FsFixture::new();
    let config = TestConnectionConfig {
        read_text_file: Some(fs.read_handler("/tmp/test_acp_read.txt", "test-read-content-12345")),
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "test-read-content-12345");
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallKind(ToolKind::Read),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    fs.assert_called();
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_fs_write_text_file_false<C: Connection>() {
    let _ = fs::remove_file("/tmp/test_acp_write.txt");

    let expected_session_id = C::expected_session_id();
    let prompt =
        "Use the write tool to write 'test-write-content-67890' to /tmp/test_acp_write.txt";
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_fs_write_tool_call.txt"),
            ),
            (
                r#"Created /tmp/test_acp_write.txt"#.into(),
                include_str!("../acp_test_data/openai_fs_write_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        builtins: vec!["developer".to_string()],
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(prompt, PermissionDecision::AllowOnce)
        .await
        .unwrap();
    assert!(!output.text.is_empty());
    assert_eq!(
        fs::read_to_string("/tmp/test_acp_write.txt").unwrap(),
        "test-write-content-67890"
    );
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_fs_write_text_file_true<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let prompt =
        "Use the write tool to write 'test-write-content-67890' to /tmp/test_acp_write.txt";
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_fs_write_tool_call.txt"),
            ),
            (
                r#"Created /tmp/test_acp_write.txt"#.into(),
                include_str!("../acp_test_data/openai_fs_write_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let fs = FsFixture::new();
    let config = TestConnectionConfig {
        builtins: vec!["developer".to_string()],
        write_text_file: Some(
            fs.write_handler("/tmp/test_acp_write.txt", "test-write-content-67890"),
        ),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(prompt, PermissionDecision::AllowOnce)
        .await
        .unwrap();
    assert!(!output.text.is_empty());

    let updates = session.session_updates();
    let initial_tool_call_id = updates
        .iter()
        .find_map(|update| match update {
            SessionUpdate::ToolCall(tool_call) => Some(&tool_call.tool_call_id),
            _ => None,
        })
        .expect("expected an initial tool call");
    for update in &updates {
        if let SessionUpdate::ToolCallUpdate(update) = update {
            assert_eq!(&update.tool_call_id, initial_tool_call_id);
        }
    }
    assert_notifications(
        &fixtures::to_notifications(&updates),
        &[
            Notification::ToolCall,
            Notification::ToolCallKind(ToolKind::Edit),
            Notification::ToolCallContent("diff".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    fs.assert_called();
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_initialize_doesnt_hit_provider<C: Connection>() {
    let provider_factory: AcpProviderFactory =
        Arc::new(|_, _, _, _| Box::pin(async { Err(anyhow::anyhow!("no provider configured")) }));

    let openai = OpenAiFixture::new(vec![], C::expected_session_id()).await;
    let config = TestConnectionConfig {
        provider_factory: Some(provider_factory),
        ..Default::default()
    };

    let _conn = C::new(config, openai).await;
}

pub async fn run_load_mode<C: Connection>() {
    let temp_dir = tempfile::tempdir().unwrap();
    let expected_session_id = C::expected_session_id();
    let prompt = "Use the get_code tool and output only its result.";
    let mcp = McpFixture::new().await;

    let config_yaml = format!(
        "GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nextensions:\n  mcp-fixture:\n    enabled: true\n    type: streamable_http\n    name: mcp-fixture\n    description: MCP fixture\n    uri: \"{}\"\n",
        mcp.url
    );
    fs::write(temp_dir.path().join(CONFIG_YAML_NAME), config_yaml).unwrap();

    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;

    let SessionData { session, modes, .. } = conn.new_session().await.unwrap();
    assert_eq!(
        modes.unwrap().current_mode_id,
        SessionModeId::new(<&str>::from(GooseMode::default()))
    );
    let session_id = session.session_id().0.to_string();
    conn.set_mode(&session_id, <&str>::from(GooseMode::Approve))
        .await
        .unwrap();

    let SessionData {
        session: mut loaded,
        modes,
        ..
    } = conn.load_session(&session_id, vec![]).await.unwrap();
    assert_eq!(
        modes.unwrap().current_mode_id,
        SessionModeId::new(<&str>::from(GooseMode::Approve))
    );

    // Approve mode + Cancel = permission denied → tool fails
    expected_session_id.set(&loaded.session_id().0);
    let output = loaded
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.tool_status.unwrap(), ToolCallStatus::Failed);
    assert_notifications(
        &loaded.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Failed),
            Notification::AgentMessage,
        ],
    );
}

pub async fn run_load_model<C: Connection>() {
    // Use a Chat Completions model so the canned SSE fixtures parse correctly.
    // TODO: add a Responses API mock to OpenAiFixture for responses-routed models.
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            r#""model":"gpt-4.1""#.into(),
            include_str!("../acp_test_data/openai_basic.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let session_id = session.session_id().0.to_string();
    conn.set_model(&session_id, "gpt-4.1").await.unwrap();

    let output = session
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");

    let SessionData { models, .. } = conn.load_session(&session_id, vec![]).await.unwrap();
    assert_eq!(models.unwrap().current_model_id, "gpt-4.1");
}

pub async fn run_load_session_mcp<C: Connection>() {
    let temp_dir = tempfile::tempdir().unwrap();
    fs::write(
        temp_dir.path().join(CONFIG_YAML_NAME),
        format!(
            "GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nextensions:\n  developer:\n    enabled: true\n    type: platform\n    name: developer\n    description: Developer\n    display_name: Developer\n    bundled: true\n    available_tools: []\n"
        ),
    )
    .unwrap();

    let expected_session_id = C::expected_session_id();
    let prompt = "Use the get_code tool and output only its result.";
    let mcp = McpFixture::new().await;
    let mcp_url = mcp.url.clone();

    // Two rounds of tool call + tool result: one for new session, one for loaded session.
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let mcp_servers = vec![McpServer::Http(McpServerHttp::new("mcp-fixture", &mcp_url))];

    let config = TestConnectionConfig {
        data_root: temp_dir.path().to_path_buf(),
        mcp_servers: mcp_servers.clone(),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);
    assert_stored_extensions(
        &conn,
        &session.session_id().0,
        &["developer", "mcp-fixture"],
    )
    .await;

    // First prompt: tool should work in the new session.
    let output = session
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, FAKE_CODE, "tool call failed in new session");

    // Load the same session with MCP servers re-specified.
    let session_id = session.session_id().0.to_string();
    let SessionData {
        session: mut loaded_session,
        ..
    } = conn.load_session(&session_id, mcp_servers).await.unwrap();
    assert_stored_extensions(&conn, &session_id, &["developer", "mcp-fixture"]).await;

    // Second prompt: tool should work in the loaded session.
    let output = loaded_session
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, FAKE_CODE, "tool call failed in loaded session");
}

async fn assert_stored_extensions<C: Connection>(conn: &C, session_id: &str, expected: &[&str]) {
    let session = SessionManager::new(conn.data_root())
        .get_session(session_id, false)
        .await
        .unwrap();
    let stored_extensions =
        EnabledExtensionsState::from_extension_data(&session.extension_data).unwrap();
    let names = stored_extensions
        .extensions
        .iter()
        .map(|extension| extension.name())
        .collect::<Vec<_>>();

    for expected_name in expected {
        assert!(
            names.iter().any(|name| name == expected_name),
            "expected stored extension {expected_name} in {names:?}"
        );
    }
}

pub async fn run_load_session_replays_image_attachment<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            r#""type":"image_url""#.into(),
            include_str!("../acp_test_data/openai_image_attachment.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);
    let session_id = session.session_id().0.to_string();

    let output = session
        .prompt_with_image(
            "Describe what you see in this image",
            TEST_IMAGE_B64,
            "image/png",
            PermissionDecision::Cancel,
        )
        .await
        .unwrap();
    assert!(output.text.contains("Hello Goose!"));
    session.session_updates();

    let SessionData { session, .. } = conn.load_session(&session_id, vec![]).await.unwrap();
    let replayed_images = session
        .session_updates()
        .into_iter()
        .filter_map(|update| match update {
            SessionUpdate::UserMessageChunk(chunk) => match chunk.content {
                ContentBlock::Image(image) => Some(image),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        replayed_images.len(),
        1,
        "expected load_session to replay the user image attachment exactly once"
    );
    let replayed_image = &replayed_images[0];
    assert_eq!(replayed_image.data, TEST_IMAGE_B64);
    assert_eq!(replayed_image.mime_type, "image/png");
}

pub async fn run_load_session_error<C: Connection>() {
    let openai = OpenAiFixture::new(vec![], C::expected_session_id()).await;
    let mut conn = C::new(TestConnectionConfig::default(), openai).await;

    let err = conn
        .load_session("nonexistent-session-id", vec![])
        .await
        .unwrap_err();

    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(
        acp_err,
        agent_client_protocol::Error::resource_not_found(Some(
            "nonexistent-session-id".to_string()
        ))
        .data("Session not found: nonexistent-session-id")
    );
}

pub async fn run_config_option_mode_set<C: Connection>() {
    run_mode_set_impl::<C>(SetModeVia::ConfigOption).await;
}

pub async fn run_config_option_set_error<C: Connection>(
    config_id: &str,
    value: &str,
    session_id_override: Option<&str>,
    expected: agent_client_protocol::Error,
) {
    let openai = OpenAiFixture::new(vec![], C::expected_session_id()).await;
    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { session, .. } = conn.new_session().await.unwrap();

    let target_session_id = session_id_override
        .map(str::to_string)
        .unwrap_or_else(|| session.session_id().0.to_string());

    let err = conn
        .set_config_option(&target_session_id, config_id, value)
        .await
        .unwrap_err();

    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(acp_err, expected);
}

#[macro_export]
macro_rules! tests_config_option_set_error {
    ($conn:ty) => {
        #[test_case::test_case("mode", "not_a_mode", None, agent_client_protocol::Error::invalid_params().data("Invalid mode: not_a_mode") ; "invalid mode via config option")]
        #[test_case::test_case("mode", "auto", Some("nonexistent-session-id"), agent_client_protocol::Error::resource_not_found(Some("nonexistent-session-id".to_string())).data("Session not found: nonexistent-session-id") ; "session not found via config option")]
        #[test_case::test_case("thought_level", "high", None, agent_client_protocol::Error::invalid_params().data("Unsupported config option: thought_level") ; "unsupported config option")]
        fn test_config_option_set_error(
            config_id: &'static str,
            value: &'static str,
            session_id: Option<&'static str>,
            expected: agent_client_protocol::Error,
        ) {
            common_tests::fixtures::run_test(async move {
                common_tests::run_config_option_set_error::<$conn>(
                    config_id, value, session_id, expected,
                )
                .await
            });
        }
    };
}

pub async fn run_mode_set<C: Connection>() {
    run_mode_set_impl::<C>(SetModeVia::Dedicated).await;
}

enum SetModeVia {
    Dedicated,
    ConfigOption,
}

async fn run_mode_set_impl<C: Connection>(via: SetModeVia) {
    let temp_dir = tempfile::tempdir().unwrap();
    let expected_session_id = C::expected_session_id();
    let prompt = "Use the get_code tool and output only its result.";
    let mcp = McpFixture::new().await;

    let config_yaml = format!(
        "GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nextensions:\n  mcp-fixture:\n    enabled: true\n    type: streamable_http\n    name: mcp-fixture\n    description: MCP fixture\n    uri: \"{}\"\n",
        mcp.url
    );
    fs::write(temp_dir.path().join(CONFIG_YAML_NAME), config_yaml).unwrap();

    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;

    let SessionData {
        session: mut session_a,
        ..
    } = conn.new_session().await.unwrap();

    let SessionData {
        session: mut session_b,
        ..
    } = conn.new_session().await.unwrap();
    let session_id = &session_b.session_id().0;
    let approve = <&str>::from(GooseMode::Approve);
    match via {
        SetModeVia::Dedicated => conn.set_mode(session_id, approve).await.unwrap(),
        SetModeVia::ConfigOption => conn
            .set_config_option(session_id, "mode", approve)
            .await
            .unwrap(),
    }

    match via {
        SetModeVia::Dedicated => {
            assert_notifications(&session_b.notifications(), &[Notification::CurrentMode])
        }
        SetModeVia::ConfigOption => {
            assert_notifications(&session_b.notifications(), &[Notification::ConfigOption])
        }
    }

    // Approve mode + Cancel = permission denied -> tool fails
    expected_session_id.set(&session_b.session_id().0);
    let output = session_b
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.tool_status.unwrap(), ToolCallStatus::Failed);
    assert_notifications(
        &session_b.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Failed),
            Notification::AgentMessage,
        ],
    );

    // Auto mode ignores Cancel -- tool succeeds without permission prompt
    conn.reset_openai();
    expected_session_id.set(&session_a.session_id().0);
    let output = session_a
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, FAKE_CODE);
    assert_notifications(
        &session_a.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
}

pub async fn run_mode_set_error<C: Connection>(
    mode_id: &str,
    session_id_override: Option<&str>,
    expected: agent_client_protocol::Error,
) {
    let openai = OpenAiFixture::new(vec![], C::expected_session_id()).await;
    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { session, .. } = conn.new_session().await.unwrap();

    let target_session_id = session_id_override
        .map(str::to_string)
        .unwrap_or_else(|| session.session_id().0.to_string());

    let err = conn
        .set_mode(&target_session_id, mode_id)
        .await
        .unwrap_err();

    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(acp_err, expected);
}

#[macro_export]
macro_rules! tests_mode_set_error {
    ($conn:ty) => {
        #[test_case::test_case("not_a_mode", None, agent_client_protocol::Error::invalid_params().data("Invalid mode: not_a_mode") ; "invalid mode")]
        #[test_case::test_case("auto", Some("nonexistent-session-id"), agent_client_protocol::Error::resource_not_found(Some("nonexistent-session-id".to_string())).data("Session not found: nonexistent-session-id") ; "session not found")]
        fn test_mode_set_error(
            mode_id: &'static str,
            session_id: Option<&'static str>,
            expected: agent_client_protocol::Error,
        ) {
            common_tests::fixtures::run_test(async move {
                common_tests::run_mode_set_error::<$conn>(
                    mode_id, session_id, expected,
                )
                .await
            });
        }
    };
}

pub async fn run_model_list<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(vec![], expected_session_id.clone()).await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData {
        session, models, ..
    } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let models = models.unwrap();
    assert!(!models.available_models.is_empty());
    assert_eq!(models.current_model_id, TEST_MODEL);
}

#[allow(dead_code)]
pub async fn run_new_session_returns_initial_config<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(vec![], expected_session_id.clone()).await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData {
        session,
        models,
        modes,
    } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    assert!(modes.is_some());
    let models = models.expect("new_session should return models inline");
    assert!(!models.available_models.is_empty());
}

pub async fn run_new_session_uses_current_config_mode<C: Connection>() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join(goose::config::base::CONFIG_YAML_NAME);
    fs::write(
        &config_path,
        format!("GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nGOOSE_MODE: approve\n"),
    )
    .unwrap();

    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(vec![], expected_session_id.clone()).await;
    let config = TestConnectionConfig {
        goose_mode: GooseMode::Approve,
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    let mut conn = C::new(config, openai).await;

    let global_config_path =
        goose::config::paths::Paths::config_dir().join(goose::config::base::CONFIG_YAML_NAME);
    fs::write(
        &global_config_path,
        format!("GOOSE_MODEL: {TEST_MODEL}\nGOOSE_PROVIDER: openai\nGOOSE_MODE: auto\n"),
    )
    .unwrap();

    let SessionData { session, modes, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    assert_eq!(modes.unwrap().current_mode_id, SessionModeId::new("auto"));
}

pub async fn run_config_option_model_set<C: Connection>() {
    run_model_set_impl::<C>().await;
}

pub async fn run_model_set<C: Connection>() {
    run_model_set_impl::<C>().await;
}

async fn run_model_set_impl<C: Connection>() {
    // Use a Chat Completions model so the canned SSE fixtures parse correctly.
    // TODO: add a Responses API mock to OpenAiFixture for responses-routed models.
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![
            // Session B prompt with switched model
            (
                r#""model":"gpt-4.1""#.into(),
                include_str!("../acp_test_data/openai_basic.txt"),
            ),
            // Session A prompt with default model
            (
                format!(r#""model":"{TEST_MODEL}""#),
                include_str!("../acp_test_data/openai_basic.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig::default();
    let mut conn = C::new(config, openai).await;

    // Session A: default model
    let SessionData {
        session: mut session_a,
        ..
    } = conn.new_session().await.unwrap();

    // Session B: switch to gpt-4.1
    let SessionData {
        session: mut session_b,
        ..
    } = conn.new_session().await.unwrap();
    let session_id = &session_b.session_id().0;
    conn.set_config_option(session_id, "model", "gpt-4.1")
        .await
        .unwrap();

    let set_model_notifs = session_b.notifications();

    // Prompt B — expects gpt-4.1
    expected_session_id.set(&session_b.session_id().0);
    let output = session_b
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");

    // Connections may emit a ConfigOption update immediately on model change,
    // or only update local state before the next prompt.
    let prompt_notifs = session_b.notifications();
    let mut all = set_model_notifs;
    all.extend(prompt_notifs);
    assert!(
        all == vec![Notification::AgentMessage]
            || all == vec![Notification::ConfigOption, Notification::AgentMessage],
        "unexpected notifications after model change: {all:?}"
    );

    // Prompt A: expects default TEST_MODEL (proves sessions are independent)
    expected_session_id.set(&session_a.session_id().0);
    let output = session_a
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");
    assert_notifications(&session_a.notifications(), &[Notification::AgentMessage]);
}

pub async fn run_model_set_error_session_not_found<C: Connection>() {
    let openai = OpenAiFixture::new(vec![], C::expected_session_id()).await;
    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { .. } = conn.new_session().await.unwrap();

    let err = conn
        .set_model("nonexistent-session-id", "o4-mini")
        .await
        .unwrap_err();

    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(
        acp_err,
        agent_client_protocol::Error::resource_not_found(Some(
            "nonexistent-session-id".to_string()
        ))
        .data("Session not found: nonexistent-session-id")
    );
}

#[allow(dead_code)]
pub async fn run_new_session_error(
    cx: &agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
    params: serde_json::Value,
    expected: agent_client_protocol::Error,
) {
    let err = fixtures::send_custom(cx, "session/new", params)
        .await
        .unwrap_err();
    assert_eq!(err, expected);
}

pub async fn run_prompt_error<C: Connection>() {
    let BasicSession { conn, mut session } =
        new_basic_session::<C>(TestConnectionConfig::default()).await;
    let sid = session.session_id().0.to_string();

    conn.delete_session(&sid).await.unwrap();

    let err = session
        .prompt("test", PermissionDecision::Cancel)
        .await
        .unwrap_err();
    let acp_err = err.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(
        acp_err.code,
        agent_client_protocol::ErrorCode::ResourceNotFound
    );
}

pub async fn run_permission_persistence<C: Connection>() {
    let cases = vec![
        (
            PermissionDecision::AllowAlways,
            ToolCallStatus::Completed,
            "user:\n  always_allow:\n  - mcp-fixture__get_code\n  ask_before: []\n  never_allow: []\n",
        ),
        (PermissionDecision::AllowOnce, ToolCallStatus::Completed, ""),
        (
            PermissionDecision::RejectAlways,
            ToolCallStatus::Failed,
            "user:\n  always_allow: []\n  ask_before: []\n  never_allow:\n  - mcp-fixture__get_code\n",
        ),
        (PermissionDecision::RejectOnce, ToolCallStatus::Failed, ""),
        (PermissionDecision::Cancel, ToolCallStatus::Failed, ""),
    ];

    let temp_dir = tempfile::tempdir().unwrap();
    let prompt = "Use the get_code tool and output only its result.";
    let expected_session_id = C::expected_session_id();
    let mcp = McpFixture::new().await;
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.to_string(),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        mcp_servers: vec![McpServer::Http(McpServerHttp::new("mcp-fixture", &mcp.url))],
        goose_mode: GooseMode::Approve,
        data_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    for (decision, expected_status, expected_yaml) in cases {
        conn.reset_openai();
        conn.reset_permissions();
        let _ = fs::remove_file(temp_dir.path().join("permission.yaml"));
        let output = session.prompt(prompt, decision).await.unwrap();

        assert_eq!(output.tool_status.unwrap(), expected_status);
        assert_eq!(
            fs::read_to_string(temp_dir.path().join("permission.yaml")).unwrap_or_default(),
            expected_yaml,
        );
    }
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_basic<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            format!("what is 1+1{TURN_CONTEXT_OPEN}"),
            include_str!("../acp_test_data/openai_basic.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");
    let updates = session.session_updates();
    let (standard_message_id, goose_message_id) = updates
        .iter()
        .find_map(|update| {
            let SessionUpdate::AgentMessageChunk(chunk) = update else {
                return None;
            };
            let standard_message_id = chunk.message_id.as_ref()?.0.to_string();
            let goose_message_id = chunk
                .meta
                .as_ref()?
                .get("goose")?
                .get("messageId")?
                .as_str()?
                .to_string();
            Some((standard_message_id, goose_message_id))
        })
        .expect("expected live agent message chunk with standard and goose message IDs");
    assert!(!standard_message_id.is_empty());
    assert_eq!(standard_message_id, goose_message_id);
    assert_notifications(
        &fixtures::to_notifications(&updates),
        &[Notification::AgentMessage],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_codemode<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let prompt =
        "Search for getCode and write tools. Use them to save the code to /tmp/result.txt.";
    let mcp = McpFixture::new().await;
    let openai = OpenAiFixture::new(
        vec![
            (
                format!("{prompt}{TURN_CONTEXT_OPEN}"),
                include_str!("../acp_test_data/openai_builtin_search.txt"),
            ),
            (
                r#"export async function getCode"#.into(),
                include_str!("../acp_test_data/openai_builtin_execute.txt"),
            ),
            (
                r#"Created /tmp/result.txt"#.into(),
                include_str!("../acp_test_data/openai_builtin_final.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        builtins: vec!["code_execution".to_string(), "developer".to_string()],
        mcp_servers: vec![McpServer::Http(McpServerHttp::new("mcp-fixture", &mcp.url))],
        ..Default::default()
    };

    let _ = fs::remove_file("/tmp/result.txt");

    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(prompt, PermissionDecision::Cancel)
        .await
        .unwrap();
    if matches!(output.tool_status, Some(ToolCallStatus::Failed)) || output.text.contains("error") {
        panic!("{}", output.text);
    }

    let result = fs::read_to_string("/tmp/result.txt").unwrap_or_default();
    assert_eq!(result, FAKE_CODE);
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_image<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let mcp = McpFixture::new().await;
    let openai = OpenAiFixture::new(
        vec![
            (
                format!(
                    "Use the get_image tool and describe what you see in its result.{TURN_CONTEXT_OPEN}"
                ),
                include_str!("../acp_test_data/openai_image_tool_call.txt"),
            ),
            (
                r#""type":"image_url""#.into(),
                include_str!("../acp_test_data/openai_image_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        mcp_servers: vec![McpServer::Http(McpServerHttp::new("mcp-fixture", &mcp.url))],
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(
            "Use the get_image tool and describe what you see in its result.",
            PermissionDecision::Cancel,
        )
        .await
        .unwrap();
    assert_eq!(output.text, "Hello Goose!\nThis is a test image.");
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_image_attachment<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            r#""type":"image_url""#.into(),
            include_str!("../acp_test_data/openai_image_attachment.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let mut conn = C::new(TestConnectionConfig::default(), openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt_with_image(
            "Describe what you see in this image",
            TEST_IMAGE_B64,
            "image/png",
            PermissionDecision::Cancel,
        )
        .await
        .unwrap();
    assert!(output.text.contains("Hello Goose!"));
    assert_notifications(&session.notifications(), &[Notification::AgentMessage]);
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_mcp<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let mcp = McpFixture::new().await;
    let openai = OpenAiFixture::new(
        vec![
            (
                format!("Use the get_code tool and output only its result.{TURN_CONTEXT_OPEN}"),
                include_str!("../acp_test_data/openai_tool_call.txt"),
            ),
            (
                format!(r#""content":"{FAKE_CODE}""#),
                include_str!("../acp_test_data/openai_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        mcp_servers: vec![McpServer::Http(McpServerHttp::new("mcp-fixture", &mcp.url))],
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(
            "Use the get_code tool and output only its result.",
            PermissionDecision::Cancel,
        )
        .await
        .unwrap();
    assert_eq!(output.text, FAKE_CODE);
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_prompt_model_mismatch<C: Connection>() {
    // Start the connection where the current model differs from TEST_MODEL.
    // Use a Chat Completions model so the canned SSE fixtures parse correctly.
    // TODO: add a Responses API mock to OpenAiFixture so we can test with
    // responses-routed models like o4-mini here.
    let config = TestConnectionConfig {
        current_model: "gpt-4o".to_string(),
        ..Default::default()
    };

    // Server starts on gpt-4o; client is configured with TEST_MODEL.
    // If session_model is seeded from the response, stream() detects the
    // mismatch and sends set_model(TEST_MODEL) before prompting.
    drop(new_basic_session::<C>(config).await);
}

pub async fn run_prompt_skill<C: Connection>() {
    let cwd = tempfile::tempdir().unwrap();
    let skill_dir = cwd.path().join(".agents/skills/test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: skill-loaded-in-acp-session\n---\nTest instructions\n",
    )
    .unwrap();

    let expected_session_id = C::expected_session_id();
    let openai = OpenAiFixture::new(
        vec![(
            "skill-loaded-in-acp-session".to_string(),
            include_str!("../acp_test_data/openai_basic.txt"),
        )],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        builtins: vec!["summon".to_string()],
        cwd: Some(cwd),
        ..Default::default()
    };

    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt("what is 1+1", PermissionDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(output.text, "2");
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_shell_terminal_false<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let prompt = format!("Run the command echo {SHELL_TEST_CONTENT} and output only its result.");
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.clone(),
                include_str!("../acp_test_data/openai_shell_tool_call.txt"),
            ),
            (
                SHELL_TEST_CONTENT.into(),
                include_str!("../acp_test_data/openai_shell_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let config = TestConnectionConfig {
        builtins: vec!["developer".to_string()],
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(&prompt, PermissionDecision::AllowOnce)
        .await
        .unwrap();
    assert!(!output.text.is_empty());
    let mut notifications = session.notifications();
    notifications.retain(|notification| {
        !matches!(
            notification,
            Notification::ToolCallStatus(ToolCallStatus::InProgress)
        )
    });
    assert_notifications(
        &notifications,
        &[
            Notification::ToolCall,
            Notification::ToolCallContent("content".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    expected_session_id.assert_matches(&session.session_id().0);
}

pub async fn run_shell_terminal_true<C: Connection>() {
    let expected_session_id = C::expected_session_id();
    let prompt = format!("Run the command echo {SHELL_TEST_CONTENT} and output only its result.");
    let openai = OpenAiFixture::new(
        vec![
            (
                prompt.clone(),
                include_str!("../acp_test_data/openai_shell_tool_call.txt"),
            ),
            (
                SHELL_TEST_CONTENT.into(),
                include_str!("../acp_test_data/openai_shell_tool_result.txt"),
            ),
        ],
        expected_session_id.clone(),
    )
    .await;

    let command = format!("echo {SHELL_TEST_CONTENT}");
    let output_text = format!("{SHELL_TEST_CONTENT}\n");
    let tid = String::from("term-1");
    let terminal = TerminalFixture::new(vec![
        TerminalCall::Create(command.clone(), tid.clone()),
        TerminalCall::WaitForExit(tid.clone(), 0),
        TerminalCall::Output(tid.clone(), output_text.clone(), 0),
        TerminalCall::Release(tid),
    ]);
    let config = TestConnectionConfig {
        builtins: vec!["developer".to_string()],
        terminal: Some(terminal.clone()),
        ..Default::default()
    };
    let mut conn = C::new(config, openai).await;
    let SessionData { mut session, .. } = conn.new_session().await.unwrap();
    expected_session_id.set(&session.session_id().0);

    let output = session
        .prompt(&prompt, PermissionDecision::AllowOnce)
        .await
        .unwrap();
    assert_eq!(output.tool_status, Some(ToolCallStatus::Completed));
    assert_notifications(
        &session.notifications(),
        &[
            Notification::ToolCall,
            Notification::ToolCallKind(ToolKind::Execute),
            Notification::ToolCallContent("terminal".into()),
            Notification::ToolCallStatus(ToolCallStatus::Completed),
            Notification::AgentMessage,
        ],
    );
    terminal.assert_called();
    expected_session_id.assert_matches(&session.session_id().0);
}
