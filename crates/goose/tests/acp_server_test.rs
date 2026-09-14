#![recursion_limit = "256"]
#[allow(dead_code)]
#[path = "acp_common_tests/mod.rs"]
mod common_tests;
use agent_client_protocol::schema::v1::{
    ContentBlock, ListSessionsRequest, ListSessionsResponse, NewSessionRequest, PromptRequest,
    SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue, SessionInfo,
    SetSessionConfigOptionRequest, StopReason, TextContent,
};
use agent_client_protocol::ErrorCode;
use common_tests::fixtures::server::{
    assert_session_response_precedes_available_commands, AcpServerConnection,
};
use common_tests::fixtures::{
    run_test, spawn_acp_server_in_process, Connection, OpenAiFixture, Session, TestConnectionConfig,
};
#[cfg(feature = "code-mode")]
use common_tests::run_prompt_codemode;
use common_tests::{
    run_close_session, run_config_mcp, run_config_option_mode_set, run_config_option_model_set,
    run_delete_session, run_fs_read_text_file_true, run_fs_write_text_file_false,
    run_fs_write_text_file_true, run_initialize_doesnt_hit_provider, run_list_sessions,
    run_load_mode, run_load_model, run_load_session_error, run_load_session_mcp,
    run_load_session_replays_image_attachment, run_mode_set, run_model_list, run_model_set,
    run_model_set_error_session_not_found, run_new_session_returns_initial_config,
    run_new_session_uses_current_config_mode, run_permission_persistence, run_prompt_basic,
    run_prompt_error, run_prompt_image, run_prompt_image_attachment, run_prompt_mcp,
    run_prompt_model_mismatch, run_prompt_skill, run_session_name_update_notification,
    run_shell_terminal_false, run_shell_terminal_true, GENERATED_SESSION_TITLE,
    OPENAI_SESSION_NAME_RESPONSE, TURN_CONTEXT_OPEN,
};
use goose::config::GooseMode;
use goose::conversation::message::{Message, MessageMetadata};
use goose::custom_requests::{
    GetSessionInfoRequest, GetSessionInfoResponse, UpdateSessionProjectRequest,
};
use goose::recipe::{Recipe, Settings};
use goose::recipe_deeplink;
use goose::session::{SessionManager, SessionType};
use std::path::Path;

tests_config_option_set_error!(AcpServerConnection);
tests_mode_set_error!(AcpServerConnection);

async fn seed_list_sessions(data_root: &Path, working_dir: &Path, count: usize) {
    let session_manager = SessionManager::new(data_root.to_path_buf());
    for index in 0..count {
        let session = session_manager
            .create_session(
                working_dir.to_path_buf(),
                format!("Seed session {index}"),
                SessionType::Acp,
                GooseMode::default(),
            )
            .await
            .unwrap();
        session_manager
            .add_message(&session.id, &Message::user().with_text("hello"))
            .await
            .unwrap();
    }
}

async fn seed_list_session_with_message(
    data_root: &Path,
    working_dir: &Path,
    name: &str,
    session_type: SessionType,
    message: &str,
) {
    let session_manager = SessionManager::new(data_root.to_path_buf());
    let session = session_manager
        .create_session(
            working_dir.to_path_buf(),
            name.to_string(),
            session_type,
            GooseMode::default(),
        )
        .await
        .unwrap();
    session_manager
        .add_message(&session.id, &Message::user().with_text(message))
        .await
        .unwrap();
}

async fn new_connection(data_root: &Path) -> AcpServerConnection {
    let openai = OpenAiFixture::new(
        vec![],
        <AcpServerConnection as Connection>::expected_session_id(),
    )
    .await;
    <AcpServerConnection as Connection>::new(
        TestConnectionConfig {
            data_root: data_root.to_path_buf(),
            ..Default::default()
        },
        openai,
    )
    .await
}

async fn list_sessions_request(
    conn: &AcpServerConnection,
    request: ListSessionsRequest,
) -> anyhow::Result<ListSessionsResponse> {
    conn.cx()
        .send_request(request)
        .block_task()
        .await
        .map_err(Into::into)
}

async fn get_session_info_request(
    conn: &AcpServerConnection,
    request: GetSessionInfoRequest,
) -> anyhow::Result<GetSessionInfoResponse> {
    conn.cx()
        .send_request(request)
        .block_task()
        .await
        .map_err(Into::into)
}

fn assert_invalid_params(error: anyhow::Error) {
    let acp_error = error.downcast::<agent_client_protocol::Error>().unwrap();
    assert_eq!(acp_error.code, ErrorCode::InvalidParams);
}

fn session_title_meta(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("sessionTitle".to_string(), value);
    meta
}

async fn new_session_with_meta(
    conn: &AcpServerConnection,
    work_dir: &Path,
    meta: serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<String> {
    let response = conn
        .cx()
        .send_request(NewSessionRequest::new(work_dir).meta(meta))
        .block_task()
        .await?;
    Ok(response.session_id.0.to_string())
}

/// Returns the session's title and whether it is recorded as user-set.
async fn session_title(conn: &AcpServerConnection, session_id: &str) -> (String, bool) {
    let response = get_session_info_request(
        conn,
        GetSessionInfoRequest {
            session_id: session_id.to_string(),
        },
    )
    .await
    .unwrap();
    let user_set_name = response
        .session
        .meta
        .as_ref()
        .and_then(|meta| meta.get("userSetName"))
        .and_then(serde_json::Value::as_bool)
        .expect("session info should include userSetName");
    (
        response
            .session
            .title
            .expect("session info should include a title"),
        user_set_name,
    )
}

fn include_last_message_snippet_meta(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let mut goose = serde_json::Map::new();
    goose.insert("includeLastMessageSnippet".to_string(), value);

    let mut meta = serde_json::Map::new();
    meta.insert("goose".to_string(), serde_json::Value::Object(goose));
    meta
}

fn last_message_snippet(session: &SessionInfo) -> Option<&str> {
    session
        .meta
        .as_ref()
        .and_then(|meta| meta.get("lastMessageSnippet"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn test_config_mcp() {
    run_test(async { run_config_mcp::<AcpServerConnection>().await });
}

#[test]
fn test_config_option_mode_set() {
    run_test(async { run_config_option_mode_set::<AcpServerConnection>().await });
}

#[test]
fn test_list_sessions() {
    run_test(async { run_list_sessions::<AcpServerConnection>().await });
}

#[test]
fn test_list_sessions_emits_computed_snippet() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/tmp/acp-session-list-snippet");
        let session_manager = SessionManager::new(data_root.path().to_path_buf());
        let session = session_manager
            .create_session(
                cwd.to_path_buf(),
                "Live subtitle".to_string(),
                SessionType::Acp,
                GooseMode::default(),
            )
            .await
            .unwrap();
        session_manager
            .add_message(
                &session.id,
                &Message::user().with_text("**raw** _markdown_ subtitle"),
            )
            .await
            .unwrap();
        session_manager
            .add_message(
                &session.id,
                &Message::assistant()
                    .with_text("hidden newer text")
                    .with_metadata(MessageMetadata::agent_only()),
            )
            .await
            .unwrap();

        let conn = new_connection(data_root.path()).await;
        let response = list_sessions_request(
            &conn,
            ListSessionsRequest::new()
                .meta(include_last_message_snippet_meta(serde_json::Value::Null)),
        )
        .await
        .unwrap();

        assert_eq!(response.sessions.len(), 1);
        assert_eq!(last_message_snippet(&response.sessions[0]), None);

        let response = list_sessions_request(
            &conn,
            ListSessionsRequest::new().meta(include_last_message_snippet_meta(
                serde_json::Value::Bool(false),
            )),
        )
        .await
        .unwrap();

        assert_eq!(response.sessions.len(), 1);
        assert_eq!(last_message_snippet(&response.sessions[0]), None);

        let response = list_sessions_request(
            &conn,
            ListSessionsRequest::new().meta(include_last_message_snippet_meta(
                serde_json::Value::Bool(true),
            )),
        )
        .await
        .unwrap();

        assert_eq!(response.sessions.len(), 1);
        assert_eq!(
            last_message_snippet(&response.sessions[0]),
            Some("**raw** _markdown_ subtitle")
        );
    });
}

#[test]
fn test_list_sessions_pagination() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        seed_list_sessions(data_root.path(), Path::new("/tmp/acp-session-list"), 51).await;
        let conn = new_connection(data_root.path()).await;

        let first = list_sessions_request(&conn, ListSessionsRequest::new())
            .await
            .unwrap();
        assert_eq!(first.sessions.len(), 50);
        assert!(first
            .sessions
            .iter()
            .all(|session| last_message_snippet(session).is_none()));

        let second = list_sessions_request(
            &conn,
            ListSessionsRequest::new()
                .cursor(first.next_cursor.clone().unwrap())
                .meta(include_last_message_snippet_meta(serde_json::Value::Bool(
                    true,
                ))),
        )
        .await
        .unwrap();
        assert_eq!(second.sessions.len(), 1);
        assert!(second.next_cursor.is_none());
        assert_eq!(last_message_snippet(&second.sessions[0]), Some("hello"));

        let second_id = &second.sessions[0].session_id;
        assert!(first
            .sessions
            .iter()
            .all(|session| session.session_id != *second_id));
    });
}

#[test]
fn test_list_sessions_query_filters_results() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/tmp/acp-session-list");
        seed_list_session_with_message(
            data_root.path(),
            cwd,
            "Postgres session",
            SessionType::Acp,
            "Discuss Postgres migrations",
        )
        .await;
        seed_list_session_with_message(
            data_root.path(),
            cwd,
            "Mobile session",
            SessionType::Acp,
            "Plan the mobile release",
        )
        .await;
        let conn = new_connection(data_root.path()).await;

        let mut meta = serde_json::Map::new();
        meta.insert(
            "query".to_string(),
            serde_json::Value::String("postgres".to_string()),
        );
        let response = list_sessions_request(&conn, ListSessionsRequest::new().meta(meta))
            .await
            .unwrap();

        assert_eq!(response.sessions.len(), 1);
        assert_eq!(
            response.sessions[0].title.as_deref(),
            Some("Postgres session")
        );
        assert!(response.next_cursor.is_none());
    });
}

#[test]
fn test_list_sessions_types_override_filters_results() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/tmp/acp-session-list");
        seed_list_session_with_message(
            data_root.path(),
            cwd,
            "ACP session",
            SessionType::Acp,
            "ACP message",
        )
        .await;
        seed_list_session_with_message(
            data_root.path(),
            cwd,
            "User session",
            SessionType::User,
            "User message",
        )
        .await;
        let conn = new_connection(data_root.path()).await;

        let mut meta = serde_json::Map::new();
        meta.insert(
            "types".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("user".to_string())]),
        );
        let response = list_sessions_request(&conn, ListSessionsRequest::new().meta(meta))
            .await
            .unwrap();

        assert_eq!(response.sessions.len(), 1);
        assert_eq!(response.sessions[0].title.as_deref(), Some("User session"));
        assert!(response.next_cursor.is_none());
    });
}

#[test]
fn test_list_sessions_types_rejects_internal_session_types() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;

        for session_type in ["hidden", "sub_agent"] {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "types".to_string(),
                serde_json::Value::Array(vec![serde_json::Value::String(session_type.to_string())]),
            );

            let error = list_sessions_request(&conn, ListSessionsRequest::new().meta(meta))
                .await
                .unwrap_err();
            assert_invalid_params(error);
        }
    });
}

#[test]
fn test_list_sessions_invalid_params() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let other_cwd = tempfile::tempdir().unwrap();
        seed_list_sessions(data_root.path(), cwd.path(), 51).await;
        let conn = new_connection(data_root.path()).await;

        let error =
            list_sessions_request(&conn, ListSessionsRequest::new().cursor("*".to_string()))
                .await
                .unwrap_err();
        assert_invalid_params(error);

        let error = list_sessions_request(
            &conn,
            ListSessionsRequest::new().cwd(std::path::PathBuf::from("relative/path")),
        )
        .await
        .unwrap_err();
        assert_invalid_params(error);

        let first = list_sessions_request(&conn, ListSessionsRequest::new().cwd(cwd.path()))
            .await
            .unwrap();

        let error = list_sessions_request(
            &conn,
            ListSessionsRequest::new()
                .cwd(other_cwd.path())
                .cursor(first.next_cursor.unwrap()),
        )
        .await
        .unwrap_err();
        assert_invalid_params(error);

        let error = list_sessions_request(
            &conn,
            ListSessionsRequest::new().meta(include_last_message_snippet_meta(
                serde_json::Value::String("true".to_string()),
            )),
        )
        .await
        .unwrap_err();
        assert_invalid_params(error);
    });
}

#[test]
fn test_get_session_info() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/tmp/acp-session-info");
        let session_manager = SessionManager::new(data_root.path().to_path_buf());
        let session = session_manager
            .create_session(
                cwd.to_path_buf(),
                "Session info".to_string(),
                SessionType::Acp,
                GooseMode::default(),
            )
            .await
            .unwrap();
        session_manager
            .add_message(&session.id, &Message::user().with_text("hello"))
            .await
            .unwrap();
        let conn = new_connection(data_root.path()).await;

        let response = get_session_info_request(
            &conn,
            GetSessionInfoRequest {
                session_id: session.id.clone(),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            response.session.session_id,
            agent_client_protocol::schema::v1::SessionId::new(session.id)
        );
        assert_eq!(response.session.cwd, cwd.to_path_buf());
        assert_eq!(response.session.title.as_deref(), Some("Session info"));
        assert!(response.session.updated_at.is_some());

        let meta = response
            .session
            .meta
            .expect("session info should include meta");
        assert!(meta.get("createdAt").and_then(|v| v.as_str()).is_some());
        assert_eq!(meta.get("messageCount"), Some(&serde_json::json!(1)));
        assert_eq!(meta.get("userSetName"), Some(&serde_json::json!(false)));
        assert_eq!(meta.get("sessionType"), Some(&serde_json::json!("acp")));
        assert_eq!(meta.get("hasRecipe"), Some(&serde_json::json!(false)));
    });
}

#[test]
fn test_update_session_project_rejects_hidden_session_types() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let session_manager = SessionManager::new(data_root.path().to_path_buf());
        let mut hidden_sessions = Vec::new();

        for session_type in [
            SessionType::Gateway,
            SessionType::SubAgent,
            SessionType::Hidden,
            SessionType::Terminal,
        ] {
            hidden_sessions.push(
                session_manager
                    .create_session(
                        working_dir.path().to_path_buf(),
                        format!("{session_type} session"),
                        session_type,
                        GooseMode::default(),
                    )
                    .await
                    .unwrap(),
            );
        }

        let conn = new_connection(data_root.path()).await;
        for session in hidden_sessions {
            let error = conn
                .cx()
                .send_request(UpdateSessionProjectRequest {
                    session_id: session.id.clone(),
                    project_id: Some("untrusted-project".to_string()),
                })
                .block_task()
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::ResourceNotFound);

            let stored = session_manager
                .get_session(&session.id, false)
                .await
                .unwrap();
            assert_eq!(stored.project_id, None);
        }
    });
}

#[test]
fn test_update_session_project_rejects_unknown_persisted_session_type() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let session_manager = SessionManager::new(data_root.path().to_path_buf());
        let session = session_manager
            .create_session(
                working_dir.path().to_path_buf(),
                "Future session".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();
        let db_path = data_root
            .path()
            .join(goose::session::session_manager::SESSIONS_FOLDER)
            .join(goose::session::session_manager::DB_NAME);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(db_path))
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET session_type = 'future_type' WHERE id = ?")
            .bind(&session.id)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            session_manager
                .get_session(&session.id, false)
                .await
                .unwrap()
                .session_type,
            SessionType::User
        );

        let conn = new_connection(data_root.path()).await;
        let error = conn
            .cx()
            .send_request(UpdateSessionProjectRequest {
                session_id: session.id.clone(),
                project_id: Some("untrusted-project".to_string()),
            })
            .block_task()
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceNotFound);

        let project_id =
            sqlx::query_scalar::<_, Option<String>>("SELECT project_id FROM sessions WHERE id = ?")
                .bind(&session.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(project_id, None);
    });
}

#[test]
fn test_update_session_project_allows_visible_session_types() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let session_manager = SessionManager::new(data_root.path().to_path_buf());
        let mut visible_sessions = Vec::new();

        for session_type in [SessionType::User, SessionType::Scheduled, SessionType::Acp] {
            visible_sessions.push(
                session_manager
                    .create_session(
                        working_dir.path().to_path_buf(),
                        format!("{session_type} session"),
                        session_type,
                        GooseMode::default(),
                    )
                    .await
                    .unwrap(),
            );
        }

        let conn = new_connection(data_root.path()).await;
        for session in visible_sessions {
            conn.cx()
                .send_request(UpdateSessionProjectRequest {
                    session_id: session.id.clone(),
                    project_id: Some("project".to_string()),
                })
                .block_task()
                .await
                .unwrap();
            assert_eq!(
                session_manager
                    .get_session(&session.id, false)
                    .await
                    .unwrap()
                    .project_id,
                Some("project".to_string())
            );

            conn.cx()
                .send_request(UpdateSessionProjectRequest {
                    session_id: session.id.clone(),
                    project_id: None,
                })
                .block_task()
                .await
                .unwrap();
            assert_eq!(
                session_manager
                    .get_session(&session.id, false)
                    .await
                    .unwrap()
                    .project_id,
                None
            );
        }
    });
}

#[test]
fn test_session_name_update_notification() {
    run_test(async { run_session_name_update_notification::<AcpServerConnection>().await });
}

#[test]
fn test_close_session() {
    run_test(async { run_close_session::<AcpServerConnection>().await });
}

#[test]
fn test_config_option_model_set() {
    run_test(async { run_config_option_model_set::<AcpServerConnection>().await });
}

#[test]
fn test_config_option_thinking_effort_set() {
    run_test(async {
        let openai = OpenAiFixture::new(
            vec![],
            <AcpServerConnection as Connection>::expected_session_id(),
        )
        .await;
        let mut conn = <AcpServerConnection as Connection>::new(
            TestConnectionConfig {
                current_model: "claude-sonnet-4".to_string(),
                ..Default::default()
            },
            openai,
        )
        .await;
        let data = conn.new_session().await.unwrap();

        let response = conn
            .cx()
            .send_request(SetSessionConfigOptionRequest::new(
                data.session.session_id().clone(),
                "thinking_effort".to_string(),
                SessionConfigOptionValue::value_id("high".to_string()),
            ))
            .block_task()
            .await
            .unwrap();

        let option = response
            .config_options
            .iter()
            .find(|option| option.id.0.as_ref() == "thinking_effort")
            .expect("thinking_effort option");
        assert_eq!(
            option.category,
            Some(SessionConfigOptionCategory::ThoughtLevel)
        );
        let select = match &option.kind {
            SessionConfigKind::Select(select) => select,
            _ => panic!("thinking_effort should be a select option"),
        };

        assert_eq!(select.current_value.0.as_ref(), "high");
    });
}

#[test]
fn test_config_option_non_value_id_returns_error_and_keeps_connection() {
    run_test(async {
        let openai = OpenAiFixture::new(
            vec![],
            <AcpServerConnection as Connection>::expected_session_id(),
        )
        .await;
        let mut conn =
            <AcpServerConnection as Connection>::new(TestConnectionConfig::default(), openai).await;
        let data = conn.new_session().await.unwrap();

        let err = conn
            .cx()
            .send_request(SetSessionConfigOptionRequest::new(
                data.session.session_id().clone(),
                "mode".to_string(),
                SessionConfigOptionValue::boolean(true),
            ))
            .block_task()
            .await
            .unwrap_err();
        assert_eq!(
            err,
            agent_client_protocol::Error::invalid_params().data("Expected a value ID")
        );

        conn.cx()
            .send_request(ListSessionsRequest::new())
            .block_task()
            .await
            .expect("connection should survive an invalid set_config_option");
    });
}

#[test]
fn test_delete_session() {
    run_test(async { run_delete_session::<AcpServerConnection>().await });
}

#[test]
fn test_fs_read_text_file_true() {
    run_test(async { run_fs_read_text_file_true::<AcpServerConnection>().await });
}

#[test]
fn test_fs_write_text_file_false() {
    run_test(async { run_fs_write_text_file_false::<AcpServerConnection>().await });
}

#[test]
fn test_fs_write_text_file_true() {
    run_test(async { run_fs_write_text_file_true::<AcpServerConnection>().await });
}

#[test]
fn test_initialize_doesnt_hit_provider() {
    run_test(async { run_initialize_doesnt_hit_provider::<AcpServerConnection>().await });
}

#[test]
fn test_load_mode() {
    run_test(async { run_load_mode::<AcpServerConnection>().await });
}

#[test]
fn test_load_model() {
    run_test(async { run_load_model::<AcpServerConnection>().await });
}

#[test]
fn test_load_session_error_session_not_found() {
    run_test(async { run_load_session_error::<AcpServerConnection>().await });
}

#[test]
fn test_load_session_mcp() {
    run_test(async { run_load_session_mcp::<AcpServerConnection>().await });
}

#[test]
fn test_load_session_replays_image_attachment() {
    run_test(async { run_load_session_replays_image_attachment::<AcpServerConnection>().await });
}

#[test]
fn test_mode_set() {
    run_test(async { run_mode_set::<AcpServerConnection>().await });
}

#[test]
fn test_model_list() {
    run_test(async { run_model_list::<AcpServerConnection>().await });
}

#[test]
fn test_new_session_returns_initial_config() {
    run_test(async { run_new_session_returns_initial_config::<AcpServerConnection>().await });
}

#[test]
fn test_new_session_uses_current_config_mode() {
    run_test(async { run_new_session_uses_current_config_mode::<AcpServerConnection>().await });
}

#[test]
fn test_new_session_response_precedes_available_commands() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let work_dir = tempfile::tempdir().unwrap();
        let openai = OpenAiFixture::new(
            vec![],
            <AcpServerConnection as Connection>::expected_session_id(),
        )
        .await;
        let (transport, _handle, _permission_manager) = spawn_acp_server_in_process(
            openai.uri(),
            &[],
            data_root.path(),
            GooseMode::default(),
            None,
            goose_test_support::TEST_MODEL,
            true,
        )
        .await;
        assert_session_response_precedes_available_commands(
            transport,
            "session/new",
            serde_json::json!({
                "cwd": work_dir.path(),
                "mcpServers": []
            }),
        )
        .await;
    });
}

#[test]
fn test_new_session_honors_recipe_model_without_recipe_provider() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();
        let recipe_model = "gpt-4.1";
        let recipe = Recipe::builder()
            .title("Recipe model")
            .description("A recipe that only overrides the model")
            .instructions("Use the requested model")
            .settings(Settings {
                goose_provider: None,
                goose_model: Some(recipe_model.to_string()),
                temperature: None,
                max_turns: None,
            })
            .build()
            .unwrap();
        let mut meta = serde_json::Map::new();
        meta.insert(
            "recipeDeeplink".to_string(),
            serde_json::Value::String(recipe_deeplink::encode(&recipe).unwrap()),
        );

        let response = conn
            .cx()
            .send_request(NewSessionRequest::new(work_dir.path()).meta(meta))
            .block_task()
            .await
            .unwrap();
        let session_info = get_session_info_request(
            &conn,
            GetSessionInfoRequest {
                session_id: response.session_id.0.to_string(),
            },
        )
        .await
        .unwrap();
        let meta = session_info
            .session
            .meta
            .expect("session info should include meta");

        assert_eq!(
            meta.get("modelId").and_then(|v| v.as_str()),
            Some(recipe_model)
        );
        assert_eq!(
            meta.get("providerId").and_then(|v| v.as_str()),
            Some("openai")
        );
    });
}

#[test]
fn test_new_session_cleans_up_when_config_fails() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();
        let mut meta = serde_json::Map::new();
        meta.insert(
            "enabledExtensions".to_string(),
            serde_json::Value::String("invalid".to_string()),
        );

        let error: anyhow::Error = conn
            .cx()
            .send_request(NewSessionRequest::new(work_dir.path()).meta(meta))
            .block_task()
            .await
            .unwrap_err()
            .into();

        assert_invalid_params(error);

        let sessions = SessionManager::new(data_root.path().to_path_buf())
            .list_all_sessions()
            .await
            .unwrap();
        assert!(sessions.is_empty());
    });
}

#[test]
fn test_new_session_titles_session_from_meta_session_title() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();

        let session_id = new_session_with_meta(
            &conn,
            work_dir.path(),
            session_title_meta(serde_json::json!("  Duncan in #general  ")),
        )
        .await
        .unwrap();

        assert_eq!(
            session_title(&conn, &session_id).await,
            ("Duncan in #general".to_string(), true)
        );
    });
}

#[test]
fn test_new_session_prefers_recipe_title_over_meta_session_title() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();
        let recipe = Recipe::builder()
            .title("Recipe title")
            .description("A recipe with a title")
            .instructions("Follow the recipe")
            .build()
            .unwrap();
        let mut meta = session_title_meta(serde_json::json!("Client title"));
        meta.insert(
            "recipeDeeplink".to_string(),
            serde_json::Value::String(recipe_deeplink::encode(&recipe).unwrap()),
        );

        let session_id = new_session_with_meta(&conn, work_dir.path(), meta)
            .await
            .unwrap();

        // The recipe title wins, and the session is not marked user-set so
        // goose's own recipe-title naming path still applies.
        assert_eq!(
            session_title(&conn, &session_id).await,
            ("Recipe title".to_string(), false)
        );
    });
}

#[test]
fn test_new_session_without_meta_session_title_uses_default_name() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();

        for meta in [
            serde_json::Map::new(),
            session_title_meta(serde_json::Value::Null),
            session_title_meta(serde_json::json!("   ")),
        ] {
            let session_id = new_session_with_meta(&conn, work_dir.path(), meta)
                .await
                .unwrap();

            assert_eq!(
                session_title(&conn, &session_id).await,
                ("New Chat".to_string(), false)
            );
        }
    });
}

#[test]
fn test_new_session_rejects_non_string_meta_session_title() {
    run_test(async {
        let data_root = tempfile::tempdir().unwrap();
        let conn = new_connection(data_root.path()).await;
        let work_dir = tempfile::tempdir().unwrap();
        let recipe = Recipe::builder()
            .title("Recipe title")
            .description("A recipe with a title")
            .instructions("Follow the recipe")
            .build()
            .unwrap();

        // Rejected on its own, and also when a recipe title would have won —
        // validation does not depend on precedence.
        let mut with_recipe = session_title_meta(serde_json::json!(42));
        with_recipe.insert(
            "recipeDeeplink".to_string(),
            serde_json::Value::String(recipe_deeplink::encode(&recipe).unwrap()),
        );
        for meta in [session_title_meta(serde_json::json!(42)), with_recipe] {
            let error = new_session_with_meta(&conn, work_dir.path(), meta)
                .await
                .unwrap_err();
            assert_invalid_params(error);
        }

        let sessions = SessionManager::new(data_root.path().to_path_buf())
            .list_all_sessions()
            .await
            .unwrap();
        assert!(sessions.is_empty());
    });
}

/// Drives one naming-enabled turn and returns the session's title once name
/// generation has had a chance to run.
async fn title_after_naming_turn(
    data_root: &Path,
    meta: serde_json::Map<String, serde_json::Value>,
) -> (String, bool) {
    let openai = OpenAiFixture::new(
        vec![
            (
                format!("what is 1+1{TURN_CONTEXT_OPEN}"),
                include_str!("acp_test_data/openai_basic.txt"),
            ),
            (
                "Generate a short title for the above messages.".to_string(),
                OPENAI_SESSION_NAME_RESPONSE,
            ),
        ],
        <AcpServerConnection as Connection>::expected_session_id(),
    )
    .await;
    let conn = <AcpServerConnection as Connection>::new(
        TestConnectionConfig {
            data_root: data_root.to_path_buf(),
            disable_session_naming: false,
            ..Default::default()
        },
        openai,
    )
    .await;
    let work_dir = tempfile::tempdir().unwrap();
    let session_id = new_session_with_meta(&conn, work_dir.path(), meta)
        .await
        .unwrap();

    let response = conn
        .cx()
        .send_request(PromptRequest::new(
            agent_client_protocol::schema::v1::SessionId::new(session_id.clone()),
            vec![ContentBlock::Text(TextContent::new("what is 1+1"))],
        ))
        .block_task()
        .await
        .unwrap();
    assert_eq!(response.stop_reason, StopReason::EndTurn);

    // Naming runs in a spawned task: wait for it to land, or for the deadline
    // to prove it never will.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let title = session_title(&conn, &session_id).await;
        if title.0 == GENERATED_SESSION_TITLE || tokio::time::Instant::now() >= deadline {
            return title;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[test]
fn test_generated_name_does_not_replace_meta_session_title() {
    run_test(async {
        // Control arm: with no client title, generation names the session.
        let data_root = tempfile::tempdir().unwrap();
        assert_eq!(
            title_after_naming_turn(data_root.path(), serde_json::Map::new()).await,
            (GENERATED_SESSION_TITLE.to_string(), false),
            "name generation must work here, or the assertion below proves nothing"
        );

        // A client title is recorded as user-set, which generation must respect.
        let data_root = tempfile::tempdir().unwrap();
        assert_eq!(
            title_after_naming_turn(
                data_root.path(),
                session_title_meta(serde_json::json!("Client title"))
            )
            .await,
            ("Client title".to_string(), true)
        );
    });
}

#[test]
fn test_model_set() {
    run_test(async { run_model_set::<AcpServerConnection>().await });
}

#[test]
fn test_model_set_error_session_not_found() {
    run_test(async { run_model_set_error_session_not_found::<AcpServerConnection>().await });
}

#[test]
fn test_permission_persistence() {
    run_test(async { run_permission_persistence::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_basic() {
    run_test(async { run_prompt_basic::<AcpServerConnection>().await });
}

#[test]
#[cfg(feature = "code-mode")]
fn test_prompt_codemode() {
    run_test(async { run_prompt_codemode::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_error_session_not_found() {
    run_test(async { run_prompt_error::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_image() {
    run_test(async { run_prompt_image::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_image_attachment() {
    run_test(async { run_prompt_image_attachment::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_mcp() {
    run_test(async { run_prompt_mcp::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_model_mismatch() {
    run_test(async { run_prompt_model_mismatch::<AcpServerConnection>().await });
}

#[test]
fn test_prompt_skill() {
    run_test(async { run_prompt_skill::<AcpServerConnection>().await });
}

#[test]
fn test_shell_terminal_false() {
    run_test(async { run_shell_terminal_false::<AcpServerConnection>().await });
}

#[test]
fn test_shell_terminal_true() {
    run_test(async { run_shell_terminal_true::<AcpServerConnection>().await });
}
