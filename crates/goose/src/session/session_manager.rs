use crate::config::paths::Paths;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageUsage, TokenState};
use crate::conversation::Conversation;
use crate::providers::base::CostSource;
use crate::providers::base::Provider;
use crate::recipe::Recipe;
use crate::session::export_markdown::export_session_to_markdown;
use crate::session::extension_data::ExtensionData;
use crate::session::session_naming::{
    generate_session_name, MSG_COUNT_FOR_SESSION_NAME_GENERATION,
};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use goose_providers::conversation::token_usage::Usage;
use goose_providers::model::ModelConfig;
use rmcp::model::Role;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, Pool, Sqlite};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tracing::{info, warn};

pub const CURRENT_SCHEMA_VERSION: i32 = 16;
pub const SESSIONS_FOLDER: &str = "sessions";
pub const DB_NAME: &str = "sessions.db";
const MILLISECOND_TIMESTAMP_THRESHOLD: i64 = 10_000_000_000;
const SESSION_COUNT_BATCH_SIZE: usize = 900;

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    Default,
    strum::Display,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SessionType {
    #[default]
    User,
    Scheduled,
    SubAgent,
    Hidden,
    Terminal,
    Gateway,
    Acp,
}

static SESSION_STORAGE: LazyLock<Arc<SessionStorage>> =
    LazyLock::new(|| Arc::new(SessionStorage::new(Paths::data_dir())));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub working_dir: PathBuf,
    #[serde(alias = "description")]
    pub name: String,
    #[serde(default)]
    pub user_set_name: bool,
    #[serde(default)]
    pub session_type: SessionType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub extension_data: ExtensionData,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub accumulated_usage: Usage,
    pub accumulated_cost: Option<f64>,
    pub schedule_id: Option<String>,
    pub recipe: Option<Recipe>,
    pub user_recipe_values: Option<HashMap<String, String>>,
    pub conversation: Option<Conversation>,
    pub message_count: usize,
    #[serde(default)]
    pub last_message_at: Option<DateTime<Utc>>,
    pub provider_name: Option<String>,
    pub model_config: Option<ModelConfig>,
    #[serde(default)]
    pub goose_mode: GooseMode,
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub last_message_snippet: Option<String>,
}

impl From<&Session> for TokenState {
    fn from(session: &Session) -> Self {
        Self {
            input_tokens: session.usage.input_tokens.unwrap_or(0),
            output_tokens: session.usage.output_tokens.unwrap_or(0),
            total_tokens: session.usage.total_tokens.unwrap_or(0),
            cache_read_tokens: session.usage.cache_read_input_tokens.unwrap_or(0),
            cache_write_tokens: session.usage.cache_write_input_tokens.unwrap_or(0),
            accumulated_input_tokens: session.accumulated_usage.input_tokens.unwrap_or(0),
            accumulated_output_tokens: session.accumulated_usage.output_tokens.unwrap_or(0),
            accumulated_total_tokens: session.accumulated_usage.total_tokens.unwrap_or(0),
            accumulated_cache_read_tokens: session
                .accumulated_usage
                .cache_read_input_tokens
                .unwrap_or(0),
            accumulated_cache_write_tokens: session
                .accumulated_usage
                .cache_write_input_tokens
                .unwrap_or(0),
            accumulated_cost: session.accumulated_cost,
        }
    }
}

pub fn token_state_from_session_and_totals(
    session: &Session,
    totals: &SessionUsageTotals,
) -> TokenState {
    TokenState {
        input_tokens: session.usage.input_tokens.unwrap_or(0),
        output_tokens: session.usage.output_tokens.unwrap_or(0),
        total_tokens: session.usage.total_tokens.unwrap_or(0),
        cache_read_tokens: session.usage.cache_read_input_tokens.unwrap_or(0),
        cache_write_tokens: session.usage.cache_write_input_tokens.unwrap_or(0),
        accumulated_input_tokens: totals.accumulated_usage.input_tokens.unwrap_or(0),
        accumulated_output_tokens: totals.accumulated_usage.output_tokens.unwrap_or(0),
        accumulated_total_tokens: totals.accumulated_usage.total_tokens.unwrap_or(0),
        accumulated_cache_read_tokens: totals
            .accumulated_usage
            .cache_read_input_tokens
            .unwrap_or(0),
        accumulated_cache_write_tokens: totals
            .accumulated_usage
            .cache_write_input_tokens
            .unwrap_or(0),
        accumulated_cost: totals.accumulated_cost,
    }
}

pub struct SessionUpdateBuilder<'a> {
    session_manager: &'a SessionManager,
    session_id: String,
    name: Option<String>,
    user_set_name: Option<bool>,
    session_type: Option<SessionType>,
    working_dir: Option<PathBuf>,
    extension_data: Option<ExtensionData>,
    usage: Option<Usage>,
    accumulated_usage: Option<Usage>,
    accumulated_cost: Option<Option<f64>>,
    schedule_id: Option<Option<String>>,
    recipe: Option<Option<Recipe>>,
    user_recipe_values: Option<Option<HashMap<String, String>>>,
    provider_name: Option<Option<String>>,
    model_config: Option<Option<ModelConfig>>,
    goose_mode: Option<GooseMode>,
    archived_at: Option<Option<DateTime<Utc>>>,

    project_id: Option<Option<String>>,
    parent_session_id: Option<Option<String>>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInsights {
    pub total_sessions: usize,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionUsageTotals {
    pub accumulated_usage: Usage,
    pub accumulated_cost: Option<f64>,
}

impl<'a> SessionUpdateBuilder<'a> {
    fn new(session_manager: &'a SessionManager, session_id: String) -> Self {
        Self {
            session_manager,
            session_id,
            name: None,
            user_set_name: None,
            session_type: None,
            working_dir: None,
            extension_data: None,
            usage: None,
            accumulated_usage: None,
            accumulated_cost: None,
            schedule_id: None,
            recipe: None,
            user_recipe_values: None,
            provider_name: None,
            model_config: None,
            goose_mode: None,
            archived_at: None,
            project_id: None,
            parent_session_id: None,
        }
    }

    pub async fn apply(self) -> Result<()> {
        self.session_manager.apply_update_inner(self).await
    }

    pub fn user_provided_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into().trim().to_string();
        if !name.is_empty() {
            self.name = Some(name);
            self.user_set_name = Some(true);
        }
        self
    }

    pub fn system_generated_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into().trim().to_string();
        if !name.is_empty() {
            self.name = Some(name);
            self.user_set_name = Some(false);
        }
        self
    }

    pub fn session_type(mut self, session_type: SessionType) -> Self {
        self.session_type = Some(session_type);
        self
    }

    pub fn working_dir(mut self, working_dir: PathBuf) -> Self {
        self.working_dir = Some(working_dir);
        self
    }

    pub fn extension_data(mut self, data: ExtensionData) -> Self {
        self.extension_data = Some(data);
        self
    }

    pub fn usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn accumulated_usage(mut self, usage: Usage) -> Self {
        self.accumulated_usage = Some(usage);
        self
    }

    pub fn accumulated_cost(mut self, cost: Option<f64>) -> Self {
        self.accumulated_cost = Some(cost);
        self
    }

    pub fn schedule_id(mut self, schedule_id: Option<String>) -> Self {
        self.schedule_id = Some(schedule_id);
        self
    }

    pub fn recipe(mut self, recipe: Option<Recipe>) -> Self {
        self.recipe = Some(recipe);
        self
    }

    pub fn user_recipe_values(
        mut self,
        user_recipe_values: Option<HashMap<String, String>>,
    ) -> Self {
        self.user_recipe_values = Some(user_recipe_values);
        self
    }

    pub fn provider_name(mut self, provider_name: impl Into<String>) -> Self {
        self.provider_name = Some(Some(provider_name.into()));
        self
    }

    pub fn model_config(mut self, model_config: ModelConfig) -> Self {
        self.model_config = Some(Some(model_config));
        self
    }

    pub fn clear_model_config(mut self) -> Self {
        self.model_config = Some(None);
        self
    }

    pub fn goose_mode(mut self, mode: GooseMode) -> Self {
        self.goose_mode = Some(mode);
        self
    }

    pub fn archived_at(mut self, archived_at: Option<DateTime<Utc>>) -> Self {
        self.archived_at = Some(archived_at);
        self
    }

    pub fn project_id(mut self, project_id: Option<String>) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn parent_session_id(mut self, parent_session_id: Option<String>) -> Self {
        self.parent_session_id = Some(parent_session_id);
        self
    }
}

pub struct SessionManager {
    storage: Arc<SessionStorage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionListCursor {
    pub(crate) sort_at: DateTime<Utc>,
    pub(crate) session_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionListPage {
    pub(crate) sessions: Vec<Session>,
    pub(crate) next_cursor: Option<SessionListCursor>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SessionListFilters<'a> {
    pub(crate) types: Option<&'a [SessionType]>,
    pub(crate) working_dir: Option<&'a Path>,
    pub(crate) keyword: Option<&'a str>,
    pub(crate) only_sessions_with_messages: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionListPageQuery<'a> {
    pub(crate) filters: SessionListFilters<'a>,
    pub(crate) cursor: Option<&'a SessionListCursor>,
    pub(crate) page_size: usize,
    pub(crate) include_last_message_snippet: bool,
}

#[derive(Debug, Default)]
struct SessionListQuery<'a> {
    filters: SessionListFilters<'a>,
    cursor: Option<&'a SessionListCursor>,
    limit: Option<usize>,
}

fn keyword_terms(query: Option<&str>) -> Vec<String> {
    query
        .unwrap_or_default()
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect()
}

fn message_keyword_clause(keyword_count: usize) -> String {
    let keyword_clauses = (0..keyword_count)
        .map(|_| "instr(LOWER(json_extract(value, '$.text')), ?) > 0")
        .collect::<Vec<_>>()
        .join(" OR ");

    let visible = user_visible_message_sql("mq.metadata_json");
    format!(
        r#"
        EXISTS (
            SELECT 1
            FROM messages mq
            WHERE mq.session_id = s.id
              AND {visible}
              AND EXISTS (
                  SELECT 1
                  FROM json_each(mq.content_json)
                  WHERE json_extract(value, '$.type') = 'text'
                    AND ({keyword_clauses})
              )
        )
        "#
    )
}

#[derive(Debug, Clone)]
pub struct SessionNameUpdate {
    pub session_id: String,
    pub name: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub user_set_name: bool,
}

impl SessionManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            storage: Arc::new(SessionStorage::new(data_dir)),
        }
    }

    pub fn instance() -> Self {
        Self {
            storage: Arc::clone(&SESSION_STORAGE),
        }
    }

    pub fn storage(&self) -> &Arc<SessionStorage> {
        &self.storage
    }

    pub(crate) fn action_required(
        &self,
    ) -> Arc<crate::action_required_manager::ActionRequiredManager> {
        self.storage.action_required.clone()
    }

    pub async fn create_session(
        &self,
        working_dir: PathBuf,
        name: String,
        session_type: SessionType,
        goose_mode: GooseMode,
    ) -> Result<Session> {
        self.storage
            .create_session(working_dir, name, session_type, goose_mode)
            .await
    }

    pub async fn get_session(&self, id: &str, include_messages: bool) -> Result<Session> {
        self.storage.get_session(id, include_messages).await
    }

    pub fn update(&self, id: &str) -> SessionUpdateBuilder<'_> {
        SessionUpdateBuilder::new(self, id.to_string())
    }

    pub(crate) async fn update_project_for_session_types(
        &self,
        id: &str,
        project_id: Option<String>,
        session_types: &[SessionType],
    ) -> Result<bool> {
        self.storage
            .update_project_for_session_types(id, project_id, session_types)
            .await
    }

    async fn apply_update_inner(&self, builder: SessionUpdateBuilder<'_>) -> Result<()> {
        self.storage.apply_update(builder).await
    }

    pub async fn add_message(&self, id: &str, message: &Message) -> Result<()> {
        self.storage.add_message(id, message).await
    }

    pub async fn replace_conversation(&self, id: &str, conversation: &Conversation) -> Result<()> {
        self.storage.replace_conversation(id, conversation).await
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_sessions().await
    }

    pub async fn list_sessions_with_limit(&self, limit: usize) -> Result<Vec<Session>> {
        self.storage
            .list_sessions_matching(SessionListQuery {
                filters: SessionListFilters {
                    types: Some(&[SessionType::User, SessionType::Scheduled]),
                    ..Default::default()
                },
                limit: Some(limit),
                ..Default::default()
            })
            .await
    }

    pub async fn list_sessions_by_types(&self, types: &[SessionType]) -> Result<Vec<Session>> {
        self.storage.list_sessions_by_types(Some(types)).await
    }

    pub(crate) async fn list_sessions_paged(
        &self,
        query: SessionListPageQuery<'_>,
    ) -> Result<SessionListPage> {
        self.storage.list_sessions_paged(query).await
    }

    pub async fn list_all_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_sessions_by_types(None).await
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        self.storage.delete_session(id).await
    }

    pub async fn get_insights(&self) -> Result<SessionInsights> {
        self.storage
            .get_insights(&[SessionType::User, SessionType::Scheduled])
            .await
    }

    pub async fn get_session_usage_totals(&self, id: &str) -> Result<SessionUsageTotals> {
        self.storage.get_session_usage_totals(id).await
    }

    pub async fn record_usage_metrics(
        &self,
        session_id: &str,
        schedule_id: Option<String>,
        current_usage: Usage,
        model: &str,
        ledger: &MessageUsage,
    ) -> Result<()> {
        self.storage
            .record_usage_metrics(session_id, schedule_id, current_usage, model, ledger)
            .await
    }

    pub async fn export_session(&self, id: &str) -> Result<String> {
        self.storage.export_session(id).await
    }

    pub async fn export_session_markdown(&self, id: &str) -> Result<String> {
        let session = self.get_session(id, true).await?;
        let messages = session
            .conversation
            .map(|conversation| conversation.user_visible_messages())
            .unwrap_or_default();
        Ok(export_session_to_markdown(messages, &session.name))
    }

    pub async fn import_session(
        &self,
        json: &str,
        session_type_override: Option<SessionType>,
    ) -> Result<Session> {
        self.storage
            .import_session(self, json, session_type_override)
            .await
    }

    pub async fn copy_session(&self, session_id: &str, new_name: String) -> Result<Session> {
        self.storage.copy_session(self, session_id, new_name).await
    }

    pub async fn truncate_conversation(&self, session_id: &str, timestamp: i64) -> Result<()> {
        self.storage
            .truncate_conversation(session_id, timestamp)
            .await
    }

    pub async fn truncate_conversation_from_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<()> {
        self.storage
            .truncate_conversation_from_message(session_id, message_id)
            .await
    }

    async fn system_generated_name_update(
        &self,
        id: &str,
        name: String,
    ) -> Result<SessionNameUpdate> {
        self.update(id)
            .system_generated_name(name.clone())
            .apply()
            .await?;

        let session = self.get_session(id, false).await?;
        Ok(SessionNameUpdate {
            session_id: id.to_string(),
            name,
            updated_at: session.updated_at,
            message_count: session.message_count,
            user_set_name: session.user_set_name,
        })
    }

    pub async fn maybe_update_name(
        &self,
        id: &str,
        provider: Arc<dyn Provider>,
    ) -> Result<Option<SessionNameUpdate>> {
        let session = self.get_session(id, true).await?;

        if session.user_set_name {
            return Ok(None);
        }

        if session.session_type == SessionType::Scheduled {
            return Ok(None);
        }

        if let Some(recipe) = &session.recipe {
            let name = recipe.title.trim().to_string();
            if name.is_empty() || session.name == name {
                return Ok(None);
            }

            return Ok(Some(self.system_generated_name_update(id, name).await?));
        }

        let model_config = match session.model_config.clone() {
            Some(model_config) => model_config,
            None => {
                let model_name =
                    crate::config::Config::global()
                        .get_goose_model()
                        .map_err(|_| {
                            anyhow::anyhow!("Could not resolve model config: missing model")
                        })?;
                crate::model_config::model_config_from_user_config(
                    provider.get_name(),
                    &model_name,
                )?
            }
        };
        let conversation = session
            .conversation
            .ok_or_else(|| anyhow::anyhow!("No messages found"))?;

        let user_message_count = conversation
            .messages()
            .iter()
            .filter(|m| matches!(m.role, Role::User) && m.is_user_visible())
            .count();

        let should_generate_name = if provider.manages_own_context() {
            user_message_count == 1
        } else {
            user_message_count <= MSG_COUNT_FOR_SESSION_NAME_GENERATION
        };

        if should_generate_name {
            let name = generate_session_name(
                provider.as_ref(),
                &model_config,
                id,
                &conversation,
                Some(session.working_dir.as_path()),
            )
            .await?;
            return Ok(Some(self.system_generated_name_update(id, name).await?));
        }
        Ok(None)
    }

    pub async fn search_chat_history(
        &self,
        query: &str,
        limit: Option<usize>,
        after_date: Option<chrono::DateTime<chrono::Utc>>,
        before_date: Option<chrono::DateTime<chrono::Utc>>,
        exclude_session_id: Option<String>,
        session_types: Vec<SessionType>,
    ) -> Result<crate::session::chat_history_search::ChatRecallResults> {
        self.storage
            .search_chat_history(
                query,
                limit,
                after_date,
                before_date,
                exclude_session_id,
                session_types,
            )
            .await
    }

    pub async fn update_message_metadata<F>(&self, id: &str, message_id: &str, f: F) -> Result<()>
    where
        F: FnOnce(
            crate::conversation::message::MessageMetadata,
        ) -> crate::conversation::message::MessageMetadata,
    {
        self.storage
            .update_message_metadata(id, message_id, f)
            .await
    }

    /// Patch `tool_meta` on a specific `ToolRequest` within a stored message.
    /// Used to persist LLM-generated tool titles and chain summaries so they
    /// survive session reload. Merge-based: existing keys not in `patch` are
    /// preserved. Searches the most recently inserted messages in the session
    /// and is a no-op if the tool_call_id is not found.
    pub async fn update_tool_request_meta(
        &self,
        session_id: &str,
        tool_call_id: &str,
        patch: serde_json::Value,
    ) -> Result<()> {
        self.storage
            .update_tool_request_meta(session_id, tool_call_id, patch)
            .await
    }
}

pub struct SessionStorage {
    pool: Pool<Sqlite>,
    initialized: tokio::sync::OnceCell<()>,
    session_dir: PathBuf,
    action_required: Arc<crate::action_required_manager::ActionRequiredManager>,
}

pub(crate) fn role_to_string(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn message_timestamp_to_datetime(timestamp: i64) -> Option<DateTime<Utc>> {
    let timestamp = if timestamp > MILLISECOND_TIMESTAMP_THRESHOLD {
        timestamp / 1000
    } else {
        timestamp
    };
    Utc.timestamp_opt(timestamp, 0).single()
}

fn normalized_message_timestamp_sql(column: &str) -> String {
    format!(
        "CASE WHEN {column} > {MILLISECOND_TIMESTAMP_THRESHOLD} THEN {column} / 1000 ELSE {column} END"
    )
}

fn user_visible_message_sql(column: &str) -> String {
    format!("COALESCE(json_extract({column}, '$.userVisible'), 1) != 0")
}

fn session_sort_at(session: &Session) -> DateTime<Utc> {
    session.last_message_at.unwrap_or(session.updated_at)
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: String::new(),
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            name: String::new(),
            user_set_name: false,
            session_type: SessionType::default(),
            created_at: Default::default(),
            updated_at: Default::default(),
            extension_data: ExtensionData::default(),
            usage: Usage::default(),
            accumulated_usage: Usage::default(),
            accumulated_cost: None,
            schedule_id: None,
            recipe: None,
            user_recipe_values: None,
            conversation: None,
            message_count: 0,
            last_message_at: None,
            provider_name: None,
            model_config: None,
            goose_mode: GooseMode::default(),
            archived_at: None,
            project_id: None,
            parent_session_id: None,
            last_message_snippet: None,
        }
    }
}

impl Session {
    pub fn without_messages(mut self) -> Self {
        self.conversation = None;
        self
    }
}

fn deserialize_session_model_config(
    provider_name: Option<&str>,
    json: &str,
) -> Option<ModelConfig> {
    let mut model_config: ModelConfig = serde_json::from_str(json).ok()?;
    // TODO: Remove this workaround once ModelConfig guarantees deserialize(serialize(config)) == config.
    if provider_name == Some(goose_providers::azure_foundry::AZURE_FOUNDRY_PROVIDER_NAME) {
        #[derive(Deserialize)]
        struct AzurePersistedFields {
            model_name: String,
            #[serde(default)]
            request_params: Option<HashMap<String, serde_json::Value>>,
        }

        let persisted: AzurePersistedFields = serde_json::from_str(json).ok()?;
        model_config.model_name = persisted.model_name;
        model_config.request_params = persisted.request_params;
    }
    Some(model_config)
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Session {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;

        let recipe_json: Option<String> = row.try_get("recipe_json")?;
        let recipe = recipe_json.and_then(|json| serde_json::from_str(&json).ok());

        let user_recipe_values_json: Option<String> = row.try_get("user_recipe_values_json")?;
        let user_recipe_values =
            user_recipe_values_json.and_then(|json| serde_json::from_str(&json).ok());

        let provider_name: Option<String> = row.try_get("provider_name").ok().flatten();
        let model_config_json: Option<String> = row.try_get("model_config_json").ok().flatten();
        let model_config = model_config_json
            .as_deref()
            .and_then(|json| deserialize_session_model_config(provider_name.as_deref(), json));

        let name: String = {
            let name_val: String = row.try_get("name").unwrap_or_default();
            if !name_val.is_empty() {
                name_val
            } else {
                row.try_get("description").unwrap_or_default()
            }
        };

        let user_set_name = row.try_get("user_set_name").unwrap_or(false);

        let session_type_str: String = row
            .try_get("session_type")
            .unwrap_or_else(|_| "user".to_string());
        let session_type = session_type_str.parse().unwrap_or_default();

        let last_message_at = row
            .try_get::<Option<i64>, _>("last_message_timestamp")
            .ok()
            .flatten()
            .and_then(message_timestamp_to_datetime);

        Ok(Session {
            id: row.try_get("id")?,
            working_dir: PathBuf::from(row.try_get::<String, _>("working_dir")?),
            name,
            user_set_name,
            session_type,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            extension_data: serde_json::from_str(&row.try_get::<String, _>("extension_data")?)
                .unwrap_or_default(),
            usage: Usage {
                input_tokens: row.try_get("input_tokens")?,
                output_tokens: row.try_get("output_tokens")?,
                total_tokens: row.try_get("total_tokens")?,
                cache_read_input_tokens: row.try_get("cache_read_tokens").ok().flatten(),
                cache_write_input_tokens: row.try_get("cache_write_tokens").ok().flatten(),
            },
            accumulated_usage: Usage {
                input_tokens: row.try_get("accumulated_input_tokens")?,
                output_tokens: row.try_get("accumulated_output_tokens")?,
                total_tokens: row.try_get("accumulated_total_tokens")?,
                cache_read_input_tokens: row
                    .try_get("accumulated_cache_read_tokens")
                    .ok()
                    .flatten(),
                cache_write_input_tokens: row
                    .try_get("accumulated_cache_write_tokens")
                    .ok()
                    .flatten(),
            },
            accumulated_cost: row.try_get("accumulated_cost").ok().flatten(),
            schedule_id: row.try_get("schedule_id")?,
            recipe,
            user_recipe_values,
            conversation: None,
            message_count: row.try_get("message_count").unwrap_or(0) as usize,
            last_message_at,
            provider_name,
            model_config,
            goose_mode: row
                .try_get::<String, _>("goose_mode")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            archived_at: row.try_get("archived_at").ok(),
            project_id: row.try_get("project_id").ok().flatten(),
            parent_session_id: row.try_get("parent_session_id").ok().flatten(),
            last_message_snippet: None,
        })
    }
}

async fn insert_usage_ledger_row(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    session_id: &str,
    model: Option<&str>,
    usage: &MessageUsage,
) -> Result<()> {
    let cost_source = usage.cost_source.map(|cs| match cs {
        CostSource::ProviderReported => "provider_reported",
        CostSource::Estimated => "estimated",
    });

    sqlx::query(
        r#"
        INSERT INTO usage_ledger (
            session_id, created_timestamp, model,
            input_tokens, output_tokens, total_tokens,
            cache_read_tokens, cache_write_tokens,
            cost, cost_source, is_compaction
        )
        VALUES (?, strftime('%s','now'), ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(session_id)
    .bind(model)
    .bind(usage.input_tokens)
    .bind(usage.output_tokens)
    .bind(usage.total_tokens)
    .bind(usage.cache_read_tokens)
    .bind(usage.cache_write_tokens)
    .bind(usage.cost)
    .bind(cost_source)
    .bind(usage.is_compaction as i64)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl SessionStorage {
    fn create_pool(path: &Path) -> Pool<Sqlite> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create session database directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                    .expect("Failed to secure session database directory");
            }
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(30))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        SqlitePoolOptions::new().connect_lazy_with(options)
    }

    pub fn new(data_dir: PathBuf) -> Self {
        let session_dir = data_dir.join(SESSIONS_FOLDER);
        let db_path = session_dir.join(DB_NAME);
        Self {
            pool: Self::create_pool(&db_path),
            initialized: tokio::sync::OnceCell::new(),
            session_dir,
            action_required: Arc::new(crate::action_required_manager::ActionRequiredManager::new()),
        }
    }

    pub(crate) async fn pool(&self) -> Result<&Pool<Sqlite>> {
        self.initialized
            .get_or_try_init(|| async {
                let schema_exists = sqlx::query_scalar::<_, bool>(
                    r#"SELECT EXISTS (SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version')"#,
                )
                .fetch_one(&self.pool)
                .await
                .unwrap_or(false);

                if schema_exists {
                    Self::run_migrations(&self.pool).await?;
                } else {
                    Self::create_schema(&self.pool).await?;
                    if let Err(e) = Self::import_legacy(&self.pool, &self.session_dir).await {
                        warn!("Failed to import some legacy sessions: {}", e);
                    }
                }
                Ok::<(), anyhow::Error>(())
            })
            .await?;
        Ok(&self.pool)
    }

    async fn create_schema(pool: &Pool<Sqlite>) -> Result<()> {
        // Run schema creation under `BEGIN IMMEDIATE` so SQLite serializes
        // writers across processes. Combined with `IF NOT EXISTS` on every
        // DDL statement and `INSERT OR IGNORE` on the bootstrap version
        // row, this makes init safe under concurrent first-run startup —
        // the previous flow:
        //
        //   SELECT EXISTS('schema_version') → false
        //   CREATE TABLE schema_version (...)
        //
        // raced when two processes both saw "doesn't exist" and the
        // second one's CREATE TABLE failed with `table already exists`,
        // which surfaced to callers as "Could not create session".
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
        "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("INSERT OR IGNORE INTO schema_version (version) VALUES (?)")
            .bind(CURRENT_SCHEMA_VERSION)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                user_set_name BOOLEAN DEFAULT FALSE,
                session_type TEXT NOT NULL DEFAULT 'user',
                working_dir TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                extension_data TEXT DEFAULT '{}',
                total_tokens INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                accumulated_total_tokens INTEGER,
                accumulated_input_tokens INTEGER,
                accumulated_output_tokens INTEGER,
                accumulated_cache_read_tokens INTEGER,
                accumulated_cache_write_tokens INTEGER,
                accumulated_cost REAL,
                schedule_id TEXT,
                recipe_json TEXT,
                user_recipe_values_json TEXT,
                provider_name TEXT,
                model_config_json TEXT,
                goose_mode TEXT NOT NULL DEFAULT 'auto',
                archived_at TIMESTAMP,
                project_id TEXT,
                parent_session_id TEXT
            )
        "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id TEXT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                created_timestamp INTEGER NOT NULL,
                timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                tokens INTEGER,
                metadata_json TEXT
            )
        "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS usage_ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                created_timestamp INTEGER NOT NULL,
                model TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                total_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                cost REAL,
                cost_source TEXT,
                is_compaction INTEGER DEFAULT 0
            )
        "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_message_id ON messages(message_id)")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_session_created ON messages(session_id, created_timestamp, id)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC)")
            .execute(&mut *tx)
            .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_type ON sessions(session_type)")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_usage_ledger_session ON usage_ledger(session_id)",
        )
        .execute(&mut *tx)
        .await?;

        // Create the inventory tables inside the same transaction so that a
        // second SessionStorage opening the same DB file never observes a
        // committed schema_version (and thus takes the migration path) while
        // the inventory tables don't yet exist — which raced as
        // `no such table: provider_inventory_entries`.
        crate::providers::inventory::create_tables(&mut tx).await?;

        tx.commit().await?;

        Ok(())
    }

    async fn import_legacy(pool: &Pool<Sqlite>, session_dir: &PathBuf) -> Result<()> {
        use crate::session::legacy;

        let sessions = match legacy::list_sessions(session_dir) {
            Ok(sessions) => sessions,
            Err(_) => {
                warn!("No legacy sessions found to import");
                return Ok(());
            }
        };

        if sessions.is_empty() {
            return Ok(());
        }

        let mut imported_count = 0;
        let mut failed_count = 0;

        for (session_name, session_path) in sessions {
            match legacy::load_session(&session_name, &session_path) {
                Ok(session) => match Self::import_legacy_session(pool, &session).await {
                    Ok(_) => {
                        imported_count += 1;
                        info!("  ✓ Imported: {}", session_name);
                    }
                    Err(e) => {
                        failed_count += 1;
                        info!("  ✗ Failed to import {}: {}", session_name, e);
                    }
                },
                Err(e) => {
                    failed_count += 1;
                    info!("  ✗ Failed to load {}: {}", session_name, e);
                }
            }
        }

        info!(
            "Import complete: {} successful, {} failed",
            imported_count, failed_count
        );
        Ok(())
    }

    async fn import_legacy_session(pool: &Pool<Sqlite>, session: &Session) -> Result<()> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let recipe_json = match &session.recipe {
            Some(recipe) => Some(serde_json::to_string(recipe)?),
            None => None,
        };

        let user_recipe_values_json = match &session.user_recipe_values {
            Some(user_recipe_values) => Some(serde_json::to_string(user_recipe_values)?),
            None => None,
        };

        let model_config_json = match &session.model_config {
            Some(model_config) => Some(serde_json::to_string(model_config)?),
            None => None,
        };

        sqlx::query(
            r#"
        INSERT INTO sessions (
            id, name, user_set_name, session_type, working_dir, created_at, updated_at, extension_data,
            total_tokens, input_tokens, output_tokens,
            cache_read_tokens, cache_write_tokens,
            accumulated_total_tokens, accumulated_input_tokens, accumulated_output_tokens,
            accumulated_cache_read_tokens, accumulated_cache_write_tokens,
            accumulated_cost,
            schedule_id, recipe_json, user_recipe_values_json,
            provider_name, model_config_json, goose_mode
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
        )
        .bind(&session.id)
        .bind(&session.name)
        .bind(session.user_set_name)
        .bind(session.session_type.to_string())
        .bind(&*session.working_dir.to_string_lossy())
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(serde_json::to_string(&session.extension_data)?)
        .bind(session.usage.total_tokens)
        .bind(session.usage.input_tokens)
        .bind(session.usage.output_tokens)
        .bind(session.usage.cache_read_input_tokens)
        .bind(session.usage.cache_write_input_tokens)
        .bind(session.accumulated_usage.total_tokens)
        .bind(session.accumulated_usage.input_tokens)
        .bind(session.accumulated_usage.output_tokens)
        .bind(session.accumulated_usage.cache_read_input_tokens)
        .bind(session.accumulated_usage.cache_write_input_tokens)
        .bind(session.accumulated_cost)
        .bind(&session.schedule_id)
        .bind(recipe_json)
        .bind(user_recipe_values_json)
        .bind(&session.provider_name)
        .bind(model_config_json)
        .bind(session.goose_mode.to_string())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        if let Some(conversation) = &session.conversation {
            Self::replace_conversation_inner(pool, &session.id, conversation).await?;
        }
        Ok(())
    }

    async fn run_migrations(pool: &Pool<Sqlite>) -> Result<()> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let current_version = Self::get_schema_version(&mut tx).await?;

        if current_version < CURRENT_SCHEMA_VERSION {
            info!(
                "Running database migrations from v{} to v{}...",
                current_version, CURRENT_SCHEMA_VERSION
            );

            for version in (current_version + 1)..=CURRENT_SCHEMA_VERSION {
                info!("  Applying migration v{}...", version);
                Self::apply_migration(&mut tx, version).await?;
                Self::update_schema_version(&mut tx, version).await?;
                info!("  ✓ Migration v{} complete", version);
            }

            info!("All migrations complete");
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_schema_version(tx: &mut sqlx::Transaction<'_, Sqlite>) -> Result<i32> {
        let table_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT name FROM sqlite_master
                WHERE type='table' AND name='schema_version'
            )
        "#,
        )
        .fetch_one(&mut **tx)
        .await?;

        if !table_exists {
            return Ok(0);
        }

        let version = sqlx::query_scalar::<_, i32>("SELECT MAX(version) FROM schema_version")
            .fetch_one(&mut **tx)
            .await?;

        Ok(version)
    }

    async fn update_schema_version(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        version: i32,
    ) -> Result<()> {
        sqlx::query("INSERT INTO schema_version (version) VALUES (?)")
            .bind(version)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn apply_migration(tx: &mut sqlx::Transaction<'_, Sqlite>, version: i32) -> Result<()> {
        match version {
            1 => {
                sqlx::query(
                    r#"
                    CREATE TABLE IF NOT EXISTS schema_version (
                        version INTEGER PRIMARY KEY,
                        applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                    )
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            2 => {
                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN user_recipe_values_json TEXT
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            3 => {
                sqlx::query(
                    r#"
                    ALTER TABLE messages ADD COLUMN metadata_json TEXT
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            4 => {
                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN name TEXT DEFAULT ''
                "#,
                )
                .execute(&mut **tx)
                .await?;

                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN user_set_name BOOLEAN DEFAULT FALSE
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            5 => {
                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN session_type TEXT NOT NULL DEFAULT 'user'
                "#,
                )
                .execute(&mut **tx)
                .await?;

                sqlx::query("CREATE INDEX idx_sessions_type ON sessions(session_type)")
                    .execute(&mut **tx)
                    .await?;
            }
            6 => {
                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN provider_name TEXT
                "#,
                )
                .execute(&mut **tx)
                .await?;

                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN model_config_json TEXT
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            7 => {
                sqlx::query(
                    r#"
                    ALTER TABLE messages ADD COLUMN message_id TEXT
                "#,
                )
                .execute(&mut **tx)
                .await?;

                sqlx::query(
                    r#"
                    UPDATE messages
                    SET message_id = 'msg_' || session_id || '_' || id
                "#,
                )
                .execute(&mut **tx)
                .await?;

                sqlx::query("CREATE INDEX idx_messages_message_id ON messages(message_id)")
                    .execute(&mut **tx)
                    .await?;
            }
            8 => {
                sqlx::query(
                    r#"
                    ALTER TABLE sessions ADD COLUMN goose_mode TEXT NOT NULL DEFAULT 'auto'
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            9 => {
                sqlx::query(
                    r#"
                    UPDATE sessions
                    SET session_type = 'acp'
                    WHERE session_type = 'user'
                      AND name = 'ACP Session'
                      AND user_set_name = FALSE
                "#,
                )
                .execute(&mut **tx)
                .await?;
            }
            10 => {
                // Check if thread_id column already exists (e.g. fresh schema)
                let has_thread_id = sqlx::query_scalar::<_, i32>(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'thread_id'",
                )
                .fetch_one(&mut **tx)
                .await?
                    > 0;
                if !has_thread_id {
                    sqlx::query("ALTER TABLE sessions ADD COLUMN thread_id TEXT")
                        .execute(&mut **tx)
                        .await?;
                }
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_sessions_thread ON sessions(thread_id)",
                )
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS threads (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL DEFAULT 'New Chat',
                        user_set_name BOOLEAN DEFAULT FALSE,
                        working_dir TEXT,
                        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                        archived_at TIMESTAMP,
                        metadata_json TEXT DEFAULT '{}'
                    )",
                )
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS thread_messages (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        thread_id TEXT NOT NULL REFERENCES threads(id),
                        session_id TEXT,
                        message_id TEXT,
                        role TEXT NOT NULL,
                        content_json TEXT NOT NULL,
                        created_timestamp INTEGER NOT NULL,
                        metadata_json TEXT DEFAULT '{}'
                    )",
                )
                .execute(&mut **tx)
                .await?;
                sqlx::query("CREATE INDEX IF NOT EXISTS idx_thread_messages_thread ON thread_messages(thread_id)")
                    .execute(&mut **tx)
                    .await?;
                sqlx::query("CREATE INDEX IF NOT EXISTS idx_thread_messages_message_id ON thread_messages(message_id)")
                    .execute(&mut **tx)
                    .await?;
            }
            11 => {
                crate::providers::inventory::create_tables(tx).await?;
            }
            12 => {
                // Add archived_at, project_id columns to sessions.
                let has_archived_at = sqlx::query_scalar::<_, i32>(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'archived_at'",
                )
                .fetch_one(&mut **tx)
                .await?
                    > 0;
                if !has_archived_at {
                    sqlx::query("ALTER TABLE sessions ADD COLUMN archived_at TIMESTAMP")
                        .execute(&mut **tx)
                        .await?;
                }

                let has_project_id = sqlx::query_scalar::<_, i32>(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'project_id'",
                )
                .fetch_one(&mut **tx)
                .await?
                    > 0;
                if !has_project_id {
                    sqlx::query("ALTER TABLE sessions ADD COLUMN project_id TEXT")
                        .execute(&mut **tx)
                        .await?;
                }
            }
            13 => {
                let has_accumulated_cost = sqlx::query_scalar::<_, i32>(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'accumulated_cost'",
                )
                .fetch_one(&mut **tx)
                .await?
                    > 0;
                if !has_accumulated_cost {
                    sqlx::query("ALTER TABLE sessions ADD COLUMN accumulated_cost REAL")
                        .execute(&mut **tx)
                        .await?;
                }
            }
            14 => {
                for column in [
                    "cache_read_tokens",
                    "cache_write_tokens",
                    "accumulated_cache_read_tokens",
                    "accumulated_cache_write_tokens",
                ] {
                    let has_column = sqlx::query_scalar::<_, i32>(
                        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?",
                    )
                    .bind(column)
                    .fetch_one(&mut **tx)
                    .await?
                        > 0;
                    if !has_column {
                        sqlx::query(AssertSqlSafe(format!(
                            "ALTER TABLE sessions ADD COLUMN {column} INTEGER"
                        )))
                        .execute(&mut **tx)
                        .await?;
                    }
                }
            }
            15 => {
                let has_parent = sqlx::query_scalar::<_, i32>(
                    "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'parent_session_id'",
                )
                .fetch_one(&mut **tx)
                .await?
                    > 0;
                if !has_parent {
                    sqlx::query("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT")
                        .execute(&mut **tx)
                        .await?;
                    sqlx::query(
                        "CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id)",
                    )
                    .execute(&mut **tx)
                    .await?;
                }

                sqlx::query(
                    r#"
                    CREATE TABLE IF NOT EXISTS usage_ledger (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                        created_timestamp INTEGER NOT NULL,
                        model TEXT,
                        input_tokens INTEGER,
                        output_tokens INTEGER,
                        total_tokens INTEGER,
                        cache_read_tokens INTEGER,
                        cache_write_tokens INTEGER,
                        cost REAL,
                        cost_source TEXT,
                        is_compaction INTEGER DEFAULT 0
                    )
                    "#,
                )
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_usage_ledger_session ON usage_ledger(session_id)",
                )
                .execute(&mut **tx)
                .await?;
            }
            16 => {
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_messages_session_created ON messages(session_id, created_timestamp, id)",
                )
                .execute(&mut **tx)
                .await?;
            }
            _ => {
                anyhow::bail!("Unknown migration version: {}", version);
            }
        }

        Ok(())
    }

    async fn create_session(
        &self,
        working_dir: PathBuf,
        name: String,
        session_type: SessionType,
        goose_mode: GooseMode,
    ) -> Result<Session> {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let today = chrono::Utc::now().format("%Y%m%d").to_string();
        let session = sqlx::query_as(
            r#"
                INSERT INTO sessions (id, name, user_set_name, session_type, working_dir, extension_data, goose_mode)
                VALUES (
                    ? || '_' || CAST(COALESCE((
                        SELECT MAX(CAST(SUBSTR(id, 10) AS INTEGER))
                        FROM sessions
                        WHERE id LIKE ? || '_%'
                    ), 0) + 1 AS TEXT),
                    ?,
                    FALSE,
                    ?,
                    ?,
                    '{}',
                    ?
                )
                RETURNING *
                "#,
        )
            .bind(&today)
            .bind(&today)
            .bind(&name)
            .bind(session_type.to_string())
            .bind(&*working_dir.to_string_lossy())
            .bind(goose_mode.to_string())
            .fetch_one(&mut *tx)
            .await?;

        tx.commit().await?;
        #[cfg(feature = "telemetry")]
        crate::posthog::emit_session_started();
        Ok(session)
    }

    async fn get_session(&self, id: &str, include_messages: bool) -> Result<Session> {
        let pool = self.pool().await?;
        let mut session = sqlx::query_as::<_, Session>(
            r#"
        SELECT id, working_dir, name, description, user_set_name, session_type, created_at, updated_at, extension_data,
               total_tokens, input_tokens, output_tokens,
               cache_read_tokens, cache_write_tokens,
               accumulated_total_tokens, accumulated_input_tokens, accumulated_output_tokens,
               accumulated_cache_read_tokens, accumulated_cache_write_tokens,
               accumulated_cost,
               schedule_id, recipe_json, user_recipe_values_json,
               provider_name, model_config_json, goose_mode,
               archived_at, project_id, parent_session_id
        FROM sessions
        WHERE id = ?
    "#,
        )
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        if include_messages {
            let conv = self.get_conversation(&session.id).await?;
            session.message_count = conv
                .messages()
                .iter()
                .filter(|m| m.is_user_visible())
                .count();
            session.last_message_at = conv
                .messages()
                .iter()
                .filter_map(|message| message_timestamp_to_datetime(message.created))
                .max();
            session.conversation = Some(conv);
        } else {
            let sql = format!(
                "SELECT COUNT(*) FILTER (WHERE {}), MAX({}) FROM messages WHERE session_id = ?",
                user_visible_message_sql("metadata_json"),
                normalized_message_timestamp_sql("created_timestamp")
            );
            let (count, last_message_timestamp): (i64, Option<i64>) =
                sqlx::query_as(AssertSqlSafe(sql))
                    .bind(&session.id)
                    .fetch_one(pool)
                    .await?;
            session.message_count = count as usize;
            session.last_message_at =
                last_message_timestamp.and_then(message_timestamp_to_datetime);
        }

        Ok(session)
    }

    #[allow(clippy::too_many_lines)]
    async fn apply_update(&self, builder: SessionUpdateBuilder<'_>) -> Result<()> {
        let mut updates = Vec::new();
        let mut query = String::from("UPDATE sessions SET ");

        macro_rules! add_update {
            ($field:expr, $name:expr) => {
                if $field.is_some() {
                    if !updates.is_empty() {
                        query.push_str(", ");
                    }
                    updates.push($name);
                    query.push_str($name);
                    query.push_str(" = ?");
                }
            };
        }

        add_update!(builder.name, "name");
        add_update!(builder.user_set_name, "user_set_name");
        add_update!(builder.session_type, "session_type");
        add_update!(builder.working_dir, "working_dir");
        add_update!(builder.extension_data, "extension_data");
        add_update!(builder.usage, "total_tokens");
        add_update!(builder.usage, "input_tokens");
        add_update!(builder.usage, "output_tokens");
        add_update!(builder.usage, "cache_read_tokens");
        add_update!(builder.usage, "cache_write_tokens");
        add_update!(builder.accumulated_usage, "accumulated_total_tokens");
        add_update!(builder.accumulated_usage, "accumulated_input_tokens");
        add_update!(builder.accumulated_usage, "accumulated_output_tokens");
        add_update!(builder.accumulated_usage, "accumulated_cache_read_tokens");
        add_update!(builder.accumulated_usage, "accumulated_cache_write_tokens");
        add_update!(builder.accumulated_cost, "accumulated_cost");
        add_update!(builder.schedule_id, "schedule_id");
        add_update!(builder.recipe, "recipe_json");
        add_update!(builder.user_recipe_values, "user_recipe_values_json");
        add_update!(builder.provider_name, "provider_name");
        add_update!(builder.model_config, "model_config_json");
        add_update!(builder.goose_mode, "goose_mode");
        add_update!(builder.archived_at, "archived_at");

        add_update!(builder.project_id, "project_id");
        add_update!(builder.parent_session_id, "parent_session_id");

        if updates.is_empty() {
            return Ok(());
        }

        query.push_str(", ");
        query.push_str("updated_at = datetime('now') WHERE id = ?");

        let mut q = sqlx::query(AssertSqlSafe(query));

        if let Some(name) = builder.name {
            q = q.bind(name);
        }
        if let Some(user_set_name) = builder.user_set_name {
            q = q.bind(user_set_name);
        }
        if let Some(session_type) = builder.session_type {
            q = q.bind(session_type.to_string());
        }
        if let Some(wd) = builder.working_dir {
            q = q.bind(wd.to_string_lossy().to_string());
        }
        if let Some(ed) = builder.extension_data {
            q = q.bind(serde_json::to_string(&ed)?);
        }
        if let Some(u) = builder.usage {
            q = q
                .bind(u.total_tokens)
                .bind(u.input_tokens)
                .bind(u.output_tokens)
                .bind(u.cache_read_input_tokens)
                .bind(u.cache_write_input_tokens);
        }
        if let Some(u) = builder.accumulated_usage {
            q = q
                .bind(u.total_tokens)
                .bind(u.input_tokens)
                .bind(u.output_tokens)
                .bind(u.cache_read_input_tokens)
                .bind(u.cache_write_input_tokens);
        }
        if let Some(ac) = builder.accumulated_cost {
            q = q.bind(ac);
        }
        if let Some(sid) = builder.schedule_id {
            q = q.bind(sid);
        }
        if let Some(recipe) = builder.recipe {
            let recipe_json = recipe.map(|r| serde_json::to_string(&r)).transpose()?;
            q = q.bind(recipe_json);
        }
        if let Some(user_recipe_values) = builder.user_recipe_values {
            let user_recipe_values_json = user_recipe_values
                .map(|urv| serde_json::to_string(&urv))
                .transpose()?;
            q = q.bind(user_recipe_values_json);
        }
        if let Some(provider_name) = builder.provider_name {
            q = q.bind(provider_name);
        }
        if let Some(model_config) = builder.model_config {
            let model_config_json = model_config
                .map(|mc| serde_json::to_string(&mc))
                .transpose()?;
            q = q.bind(model_config_json);
        }
        if let Some(goose_mode) = builder.goose_mode {
            q = q.bind(goose_mode.to_string());
        }
        if let Some(ref archived_at) = builder.archived_at {
            q = q.bind(archived_at.as_ref());
        }

        if let Some(ref project_id) = builder.project_id {
            q = q.bind(project_id.as_ref());
        }
        if let Some(ref parent_session_id) = builder.parent_session_id {
            q = q.bind(parent_session_id.as_ref());
        }

        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        q = q.bind(&builder.session_id);
        let result = q.execute(&mut *tx).await?;

        if result.rows_affected() == 0 {
            return Err(anyhow::anyhow!("Session not found: {}", builder.session_id));
        }

        tx.commit().await?;
        Ok(())
    }

    async fn update_project_for_session_types(
        &self,
        id: &str,
        project_id: Option<String>,
        session_types: &[SessionType],
    ) -> Result<bool> {
        if session_types.is_empty() {
            return Ok(false);
        }

        let placeholders = session_types
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "UPDATE sessions SET project_id = ?, updated_at = datetime('now') \
             WHERE id = ? AND session_type IN ({placeholders})"
        );
        let pool = self.pool().await?;
        let mut query = sqlx::query(AssertSqlSafe(query)).bind(project_id).bind(id);
        for session_type in session_types {
            query = query.bind(session_type.to_string());
        }

        Ok(query.execute(pool).await?.rows_affected() == 1)
    }

    async fn get_conversation(&self, session_id: &str) -> Result<Conversation> {
        let pool = self.pool().await?;
        let rows = sqlx::query_as::<_, (String, String, i64, Option<String>, Option<String>)>(
            // Order by created_timestamp, then by id to break ties. created_timestamp is in seconds,
            // so messages created in the same second (e.g., tool request and response) need to
            // maintain their insertion order via the auto-increment id.
            "SELECT role, content_json, created_timestamp, metadata_json, message_id FROM messages WHERE session_id = ? ORDER BY created_timestamp, id",
        )
            .bind(session_id)
            .fetch_all(pool)
            .await?;

        let mut messages = Vec::new();
        for (role_str, content_json, created_timestamp, metadata_json, message_id) in
            rows.into_iter()
        {
            let role = match role_str.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => continue,
            };

            let content = serde_json::from_str(&content_json)?;
            let metadata = metadata_json
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();

            let mut message = Message::new(role, created_timestamp, content);
            message.metadata = metadata;
            if let Some(id) = message_id {
                message = message.with_id(id);
            }
            messages.push(message);
        }

        Ok(Conversation::new_unvalidated(messages))
    }

    async fn add_message(&self, session_id: &str, message: &Message) -> Result<()> {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let metadata_json = serde_json::to_string(&message.metadata)?;
        // Messages are read back ordered by (created_timestamp, id), so one built
        // before the messages it is appended after would sort ahead of them —
        // operations do that whenever they prepare a reply and fill it in while a
        // tool runs. Never move a message ahead of what is already stored.
        let latest: Option<i64> =
            sqlx::query_scalar("SELECT MAX(created_timestamp) FROM messages WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;
        let created = message.created.max(latest.unwrap_or(message.created));

        let message_id = message
            .id
            .clone()
            .unwrap_or_else(|| format!("msg_{}_{}", session_id, uuid::Uuid::new_v4()));

        sqlx::query(
            r#"
            INSERT INTO messages (message_id, session_id, role, content_json, created_timestamp, metadata_json)
            VALUES (?, ?, ?, ?, ?, ?)
        "#,
        )
        .bind(message_id)
        .bind(session_id)
        .bind(role_to_string(&message.role))
        .bind(serde_json::to_string(&message.content)?)
        .bind(created)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE sessions SET updated_at = datetime('now') WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn replace_conversation_inner(
        pool: &Pool<Sqlite>,
        session_id: &str,
        conversation: &Conversation,
    ) -> Result<()> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        for message in conversation.messages() {
            let metadata_json = serde_json::to_string(&message.metadata)?;

            let message_id = message
                .id
                .clone()
                .unwrap_or_else(|| format!("msg_{}_{}", session_id, uuid::Uuid::new_v4()));

            sqlx::query(
                r#"
            INSERT INTO messages (message_id, session_id, role, content_json, created_timestamp, metadata_json)
            VALUES (?, ?, ?, ?, ?, ?)
        "#,
            )
            .bind(message_id)
            .bind(session_id)
            .bind(role_to_string(&message.role))
            .bind(serde_json::to_string(&message.content)?)
            .bind(message.created)
            .bind(metadata_json)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn replace_conversation(
        &self,
        session_id: &str,
        conversation: &Conversation,
    ) -> Result<()> {
        let pool = self.pool().await?;
        Self::replace_conversation_inner(pool, session_id, conversation).await
    }

    async fn list_sessions_matching(&self, query: SessionListQuery<'_>) -> Result<Vec<Session>> {
        let filters = &query.filters;
        if matches!(filters.types, Some(types) if types.is_empty()) {
            return Ok(Vec::new());
        }

        let has_limit = query.limit.is_some();
        let keywords = keyword_terms(filters.keyword);
        let mut where_clauses = Vec::new();
        let mut having_clauses = Vec::new();
        let normalized_message_timestamp = normalized_message_timestamp_sql("m.created_timestamp");
        let sort_timestamp_sql =
            format!("COALESCE(MAX({normalized_message_timestamp}), unixepoch(s.updated_at))");
        if let Some(types) = filters.types {
            let placeholders = types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            where_clauses.push(format!("s.session_type IN ({})", placeholders));
        }
        if filters.working_dir.is_some() {
            where_clauses.push("s.working_dir = ?".to_string());
        }
        if !keywords.is_empty() {
            where_clauses.push(message_keyword_clause(keywords.len()));
        }
        if query.cursor.is_some() {
            having_clauses.push(format!(
                "({sort_timestamp_sql} < ? OR ({sort_timestamp_sql} = ? AND s.id < ?))"
            ));
        }

        let where_clause = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };
        let having_clause = if having_clauses.is_empty() {
            String::new()
        } else {
            format!("HAVING {}", having_clauses.join(" AND "))
        };
        let message_join = if filters.only_sessions_with_messages {
            "JOIN messages m ON s.id = m.session_id"
        } else {
            "LEFT JOIN messages m ON s.id = m.session_id"
        };
        let order_by = "ORDER BY sort_timestamp DESC, s.id DESC";
        let limit_clause = if query.limit.is_some() { "LIMIT ?" } else { "" };

        let message_count_sql = if has_limit {
            "0".to_string()
        } else {
            format!(
                "COUNT(m.id) FILTER (WHERE {})",
                user_visible_message_sql("m.metadata_json")
            )
        };
        let sql = format!(
            r#"
            SELECT s.id, s.working_dir, s.name, s.description, s.user_set_name, s.session_type, s.created_at, s.updated_at, s.extension_data,
                   s.total_tokens, s.input_tokens, s.output_tokens,
                   s.cache_read_tokens, s.cache_write_tokens,
                   s.accumulated_total_tokens, s.accumulated_input_tokens, s.accumulated_output_tokens,
                   s.accumulated_cache_read_tokens, s.accumulated_cache_write_tokens,
                   s.accumulated_cost,
                   s.schedule_id, s.recipe_json, s.user_recipe_values_json,
                   s.provider_name, s.model_config_json, s.goose_mode,
                   s.archived_at, s.project_id, s.parent_session_id,
                   {} as message_count,
                   MAX({}) as last_message_timestamp,
                   {} as sort_timestamp
            FROM sessions s
            {}
            {}
            GROUP BY s.id
            {}
            {}
            {}
            "#,
            message_count_sql,
            normalized_message_timestamp,
            sort_timestamp_sql,
            message_join,
            where_clause,
            having_clause,
            order_by,
            limit_clause
        );

        let mut q = sqlx::query_as::<_, Session>(AssertSqlSafe(sql));
        if let Some(types) = filters.types {
            for session_type in types {
                q = q.bind(session_type.to_string());
            }
        }
        if let Some(working_dir) = filters.working_dir {
            q = q.bind(working_dir.to_string_lossy().to_string());
        }
        for term in keywords {
            q = q.bind(term);
        }
        if let Some(cursor) = query.cursor {
            let sort_at = cursor.sort_at.timestamp();
            q = q.bind(sort_at);
            q = q.bind(sort_at);
            q = q.bind(&cursor.session_id);
        }
        if let Some(limit) = query.limit {
            q = q.bind(limit as i64);
        }

        let pool = self.pool().await?;
        if has_limit {
            let mut tx = pool.begin().await?;
            let mut sessions = q.fetch_all(&mut *tx).await?;
            Self::populate_visible_message_counts(&mut tx, &mut sessions).await?;
            tx.commit().await?;
            Ok(sessions)
        } else {
            q.fetch_all(pool).await.map_err(Into::into)
        }
    }

    async fn populate_visible_message_counts(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        sessions: &mut [Session],
    ) -> Result<()> {
        let mut counts = HashMap::with_capacity(sessions.len());

        for chunk in sessions.chunks(SESSION_COUNT_BATCH_SIZE) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                r#"
                SELECT m.session_id,
                       COUNT(m.id) FILTER (WHERE {}) as message_count
                FROM messages m
                WHERE m.session_id IN ({})
                GROUP BY m.session_id
                "#,
                user_visible_message_sql("m.metadata_json"),
                placeholders
            );
            let mut q = sqlx::query_as::<_, (String, i64)>(AssertSqlSafe(sql));
            for session in chunk {
                q = q.bind(&session.id);
            }
            for (session_id, message_count) in q.fetch_all(&mut **tx).await? {
                counts.insert(session_id, message_count as usize);
            }
        }

        for session in sessions {
            session.message_count = counts.get(&session.id).copied().unwrap_or_default();
        }

        Ok(())
    }

    async fn list_sessions_by_types(&self, types: Option<&[SessionType]>) -> Result<Vec<Session>> {
        self.list_sessions_matching(SessionListQuery {
            filters: SessionListFilters {
                types,
                ..Default::default()
            },
            ..Default::default()
        })
        .await
    }

    async fn list_sessions_paged(
        &self,
        query: SessionListPageQuery<'_>,
    ) -> Result<SessionListPage> {
        if matches!(query.filters.types, Some(types) if types.is_empty()) || query.page_size == 0 {
            return Ok(SessionListPage {
                sessions: Vec::new(),
                next_cursor: None,
            });
        }

        let page_size = query.page_size;
        let include_last_message_snippet = query.include_last_message_snippet;
        let mut sessions = self
            .list_sessions_matching(SessionListQuery {
                filters: query.filters,
                cursor: query.cursor,
                limit: Some(page_size + 1),
            })
            .await?;
        let has_next_page = sessions.len() > page_size;
        let next_cursor = if has_next_page {
            let anchor = &sessions[page_size - 1];
            Some(SessionListCursor {
                sort_at: session_sort_at(anchor),
                session_id: anchor.id.clone(),
            })
        } else {
            None
        };
        if has_next_page {
            sessions.truncate(page_size);
        }
        if include_last_message_snippet {
            let pool = self.pool().await?;
            super::last_message_snippet::hydrate_last_message_snippets(pool, &mut sessions).await?;
        }

        Ok(SessionListPage {
            sessions,
            next_cursor,
        })
    }

    async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.list_sessions_by_types(Some(&[SessionType::User, SessionType::Scheduled]))
            .await
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)")
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;

        if !exists {
            return Err(anyhow::anyhow!("Session not found"));
        }

        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM usage_ledger WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn get_insights(&self, types: &[SessionType]) -> Result<SessionInsights> {
        if types.is_empty() {
            return Ok(SessionInsights {
                total_sessions: 0,
                total_tokens: 0,
            });
        }

        let placeholders: String = types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let query = format!(
            r#"
            SELECT COUNT(*) as total_sessions,
                   COALESCE(SUM(COALESCE(accumulated_total_tokens, total_tokens, 0)), 0) as total_tokens
            FROM sessions
            WHERE session_type IN ({})
            "#,
            placeholders
        );

        let pool = self.pool().await?;
        let mut q = sqlx::query_as::<_, (i64, Option<i64>)>(AssertSqlSafe(query));
        for t in types {
            q = q.bind(t.to_string());
        }

        let row = q.fetch_one(pool).await?;

        Ok(SessionInsights {
            total_sessions: row.0 as usize,
            total_tokens: row.1.unwrap_or(0),
        })
    }

    async fn record_usage_metrics(
        &self,
        session_id: &str,
        schedule_id: Option<String>,
        current_usage: Usage,
        model: &str,
        ledger: &MessageUsage,
    ) -> Result<()> {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        sqlx::query(
            r#"
            INSERT INTO usage_ledger (
                session_id, created_timestamp,
                input_tokens, output_tokens, total_tokens,
                cache_read_tokens, cache_write_tokens,
                cost, cost_source
            )
            SELECT s.id, strftime('%s','now'),
                   MAX(COALESCE(s.accumulated_input_tokens, 0) - l.input_sum, 0),
                   MAX(COALESCE(s.accumulated_output_tokens, 0) - l.output_sum, 0),
                   MAX(COALESCE(s.accumulated_total_tokens, 0) - l.total_sum, 0),
                   MAX(COALESCE(s.accumulated_cache_read_tokens, 0) - l.cache_read_sum, 0),
                   MAX(COALESCE(s.accumulated_cache_write_tokens, 0) - l.cache_write_sum, 0),
                   CASE WHEN s.accumulated_cost IS NULL OR s.accumulated_cost <= l.cost_sum THEN NULL
                        ELSE s.accumulated_cost - l.cost_sum END,
                   'carried_forward'
            FROM sessions s,
                 (SELECT COALESCE(SUM(input_tokens), 0) AS input_sum,
                         COALESCE(SUM(output_tokens), 0) AS output_sum,
                         COALESCE(SUM(total_tokens), 0) AS total_sum,
                         COALESCE(SUM(cache_read_tokens), 0) AS cache_read_sum,
                         COALESCE(SUM(cache_write_tokens), 0) AS cache_write_sum,
                         COALESCE(SUM(cost), 0.0) AS cost_sum
                  FROM usage_ledger WHERE session_id = ?) l
            WHERE s.id = ?
              AND (COALESCE(s.accumulated_input_tokens, 0) > l.input_sum
                   OR COALESCE(s.accumulated_output_tokens, 0) > l.output_sum
                   OR COALESCE(s.accumulated_total_tokens, 0) > l.total_sum
                   OR COALESCE(s.accumulated_cost, 0.0) > l.cost_sum + 1e-9)
            "#,
        )
        .bind(session_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            UPDATE sessions SET
                schedule_id = ?,
                total_tokens = ?, input_tokens = ?, output_tokens = ?,
                cache_read_tokens = ?, cache_write_tokens = ?,
                accumulated_total_tokens = COALESCE(accumulated_total_tokens, 0) + ?,
                accumulated_input_tokens = COALESCE(accumulated_input_tokens, 0) + ?,
                accumulated_output_tokens = COALESCE(accumulated_output_tokens, 0) + ?,
                accumulated_cache_read_tokens = COALESCE(accumulated_cache_read_tokens, 0) + ?,
                accumulated_cache_write_tokens = COALESCE(accumulated_cache_write_tokens, 0) + ?,
                accumulated_cost = CASE
                    WHEN ? IS NULL THEN accumulated_cost
                    ELSE COALESCE(accumulated_cost, 0) + ?
                END,
                updated_at = datetime('now')
            WHERE id = ?
            "#,
        )
        .bind(schedule_id)
        .bind(current_usage.total_tokens)
        .bind(current_usage.input_tokens)
        .bind(current_usage.output_tokens)
        .bind(current_usage.cache_read_input_tokens)
        .bind(current_usage.cache_write_input_tokens)
        .bind(ledger.total_tokens.unwrap_or(0))
        .bind(ledger.input_tokens.unwrap_or(0))
        .bind(ledger.output_tokens.unwrap_or(0))
        .bind(ledger.cache_read_tokens.unwrap_or(0))
        .bind(ledger.cache_write_tokens.unwrap_or(0))
        .bind(ledger.cost)
        .bind(ledger.cost)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        insert_usage_ledger_row(&mut tx, session_id, Some(model), ledger).await?;

        tx.commit().await?;
        Ok(())
    }

    async fn get_session_usage_totals(&self, session_id: &str) -> Result<SessionUsageTotals> {
        let pool = self.pool().await?;
        let rows = sqlx::query_as::<
            _,
            (
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<f64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<f64>,
            ),
        >(
            r#"
            WITH RECURSIVE tree(id) AS (
                SELECT id FROM sessions WHERE id = ?
                UNION
                SELECT s.id FROM sessions s JOIN tree ON s.parent_session_id = tree.id
            )
            SELECT
                s.accumulated_input_tokens, s.accumulated_output_tokens, s.accumulated_total_tokens,
                s.accumulated_cache_read_tokens, s.accumulated_cache_write_tokens, s.accumulated_cost,
                SUM(u.input_tokens), SUM(u.output_tokens), SUM(u.total_tokens),
                SUM(u.cache_read_tokens), SUM(u.cache_write_tokens), SUM(u.cost)
            FROM sessions s
            LEFT JOIN usage_ledger u ON u.session_id = s.id
            WHERE s.id IN (SELECT id FROM tree)
            GROUP BY s.id
            "#,
        )
        .bind(session_id)
        .fetch_all(pool)
        .await?;

        let mut input = 0i64;
        let mut output = 0i64;
        let mut total = 0i64;
        let mut cache_read = 0i64;
        let mut cache_write = 0i64;
        let mut cost: Option<f64> = None;

        let larger =
            |acc: Option<i64>, ledger: Option<i64>| acc.unwrap_or(0).max(ledger.unwrap_or(0));

        for row in rows {
            let (
                acc_in,
                acc_out,
                acc_total,
                acc_cr,
                acc_cw,
                acc_cost,
                l_in,
                l_out,
                l_total,
                l_cr,
                l_cw,
                l_cost,
            ) = row;
            input += larger(acc_in, l_in);
            output += larger(acc_out, l_out);
            total += larger(acc_total, l_total);
            cache_read += larger(acc_cr, l_cr);
            cache_write += larger(acc_cw, l_cw);
            if acc_cost.is_some() || l_cost.is_some() {
                let c = acc_cost.unwrap_or(0.0).max(l_cost.unwrap_or(0.0));
                cost = Some(cost.unwrap_or(0.0) + c);
            }
        }

        let opt = |v: i64| Some(i32::try_from(v).unwrap_or(i32::MAX));
        Ok(SessionUsageTotals {
            accumulated_usage: Usage::new(opt(input), opt(output), opt(total))
                .with_cache_tokens(opt(cache_read), opt(cache_write)),
            accumulated_cost: cost,
        })
    }

    async fn export_session(&self, id: &str) -> Result<String> {
        let session = self.get_session(id, true).await?;
        serde_json::to_string_pretty(&session).map_err(Into::into)
    }

    async fn import_session(
        &self,
        session_manager: &SessionManager,
        json: &str,
        session_type_override: Option<SessionType>,
    ) -> Result<Session> {
        let normalized = super::import_formats::convert_to_goose_session_json(json)?;
        let import: Session = serde_json::from_str(&normalized)?;

        let session = self
            .create_session(
                import.working_dir.clone(),
                import.name.clone(),
                session_type_override.unwrap_or(import.session_type),
                import.goose_mode,
            )
            .await?;

        let mut builder = session_manager
            .update(&session.id)
            .extension_data(import.extension_data)
            .usage(import.usage)
            .accumulated_usage(import.accumulated_usage)
            .accumulated_cost(import.accumulated_cost)
            .schedule_id(import.schedule_id)
            .recipe(import.recipe)
            .user_recipe_values(import.user_recipe_values);

        if import.user_set_name {
            builder = builder.user_provided_name(import.name.clone());
        }

        builder.apply().await?;

        if let Some(conversation) = import.conversation {
            self.replace_conversation(&session.id, &conversation)
                .await?;
        }

        self.get_session(&session.id, true).await
    }

    async fn copy_session(
        &self,
        session_manager: &SessionManager,
        session_id: &str,
        new_name: String,
    ) -> Result<Session> {
        let original_session = self.get_session(session_id, true).await?;

        let new_session = self
            .create_session(
                original_session.working_dir.clone(),
                new_name,
                original_session.session_type,
                original_session.goose_mode,
            )
            .await?;

        let mut builder = session_manager
            .update(&new_session.id)
            .extension_data(original_session.extension_data)
            .schedule_id(original_session.schedule_id)
            .recipe(original_session.recipe)
            .user_recipe_values(original_session.user_recipe_values);

        if let Some(project_id) = original_session.project_id {
            builder = builder.project_id(Some(project_id));
        }
        if let Some(provider_name) = original_session.provider_name {
            builder = builder.provider_name(provider_name);
        }
        if let Some(model_config) = original_session.model_config {
            builder = builder.model_config(model_config);
        }
        builder = builder.goose_mode(original_session.goose_mode);

        builder.apply().await?;

        if let Some(conversation) = original_session.conversation {
            self.replace_conversation(&new_session.id, &conversation)
                .await?;
        }

        self.get_session(&new_session.id, true).await
    }

    async fn truncate_conversation(&self, session_id: &str, timestamp: i64) -> Result<()> {
        let pool = self.pool().await?;
        sqlx::query("DELETE FROM messages WHERE session_id = ? AND created_timestamp >= ?")
            .bind(session_id)
            .bind(timestamp)
            .execute(pool)
            .await?;

        Ok(())
    }

    async fn truncate_conversation_from_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<()> {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let boundary = sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, created_timestamp FROM messages WHERE session_id = ? AND message_id = ? ORDER BY created_timestamp, id LIMIT 1",
        )
        .bind(session_id)
        .bind(message_id)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((boundary_id, boundary_timestamp)) = boundary {
            sqlx::query(
                "DELETE FROM messages WHERE session_id = ? AND (created_timestamp > ? OR (created_timestamp = ? AND id >= ?))",
            )
            .bind(session_id)
            .bind(boundary_timestamp)
            .bind(boundary_timestamp)
            .bind(boundary_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn search_chat_history(
        &self,
        query: &str,
        limit: Option<usize>,
        after_date: Option<chrono::DateTime<chrono::Utc>>,
        before_date: Option<chrono::DateTime<chrono::Utc>>,
        exclude_session_id: Option<String>,
        session_types: Vec<SessionType>,
    ) -> Result<crate::session::chat_history_search::ChatRecallResults> {
        use crate::session::chat_history_search::ChatHistorySearch;

        let pool = self.pool().await?;
        ChatHistorySearch::new(
            pool,
            query,
            limit,
            after_date,
            before_date,
            exclude_session_id,
            session_types,
        )
        .execute()
        .await
    }

    async fn update_message_metadata<F>(
        &self,
        session_id: &str,
        message_id: &str,
        f: F,
    ) -> Result<()>
    where
        F: FnOnce(
            crate::conversation::message::MessageMetadata,
        ) -> crate::conversation::message::MessageMetadata,
    {
        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let current_metadata_json = sqlx::query_scalar::<_, String>(
            "SELECT metadata_json FROM messages WHERE message_id = ? AND session_id = ?",
        )
        .bind(message_id)
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;

        let current_metadata: crate::conversation::message::MessageMetadata =
            serde_json::from_str(&current_metadata_json)?;

        let new_metadata = f(current_metadata);
        let metadata_json = serde_json::to_string(&new_metadata)?;

        sqlx::query(
            "UPDATE messages SET metadata_json = ? WHERE message_id = ? AND session_id = ?",
        )
        .bind(metadata_json)
        .bind(message_id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn update_tool_request_meta(
        &self,
        session_id: &str,
        tool_call_id: &str,
        patch: serde_json::Value,
    ) -> Result<()> {
        use crate::conversation::message::MessageContent;

        let pool = self.pool().await?;
        let rows = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT message_id, content_json FROM messages \
             WHERE session_id = ? \
             ORDER BY id DESC \
             LIMIT 100",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await?;

        for (message_id, content_json) in rows {
            let content: Vec<MessageContent> = serde_json::from_str(&content_json)?;
            let contains_tool_request = content.iter().any(|block| {
                matches!(
                    block,
                    MessageContent::ToolRequest(tool_request)
                        if tool_request.id == tool_call_id
                )
            });
            if contains_tool_request {
                let Some(message_id) = message_id else {
                    return Ok(());
                };
                return self
                    .update_tool_request_meta_by_message_id(
                        session_id,
                        &message_id,
                        tool_call_id,
                        patch,
                    )
                    .await;
            }
        }

        Ok(())
    }

    /// Patch `tool_meta` on a specific `ToolRequest` within a stored message's
    /// `content_json`. Finds the row(s) with matching `message_id`, scans each
    /// row's content for a `ToolRequest` with the given `tool_call_id`, and
    /// merges `patch` into its `tool_meta`. Uses `BEGIN IMMEDIATE` so
    /// concurrent writers serialize correctly.
    async fn update_tool_request_meta_by_message_id(
        &self,
        session_id: &str,
        message_id: &str,
        tool_call_id: &str,
        patch: serde_json::Value,
    ) -> Result<()> {
        use crate::conversation::message::MessageContent;

        let pool = self.pool().await?;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, content_json FROM messages \
             WHERE session_id = ? AND message_id = ? \
             ORDER BY id ASC",
        )
        .bind(session_id)
        .bind(message_id)
        .fetch_all(&mut *tx)
        .await?;

        for (row_id, content_json) in rows {
            let mut content: Vec<MessageContent> = serde_json::from_str(&content_json)?;
            let mut found = false;
            for block in &mut content {
                if let MessageContent::ToolRequest(tr) = block {
                    if tr.id == tool_call_id {
                        tr.tool_meta = Some(merge_tool_meta(tr.tool_meta.take(), &patch));
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                continue;
            }

            let updated_json = serde_json::to_string(&content)?;
            sqlx::query("UPDATE messages SET content_json = ? WHERE id = ?")
                .bind(updated_json)
                .bind(row_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(());
        }

        tx.commit().await?;
        Ok(())
    }
}

/// Merge a JSON object `patch` into an existing optional object value,
/// preserving keys not present in the patch.
fn merge_tool_meta(
    existing: Option<serde_json::Value>,
    patch: &serde_json::Value,
) -> serde_json::Value {
    let mut base = match existing {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    if let serde_json::Value::Object(patch_map) = patch {
        for (k, v) in patch_map {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::{Message, MessageContent};
    use crate::providers::base::MessageStream;
    use goose_providers::conversation::token_usage::{CostSource, ProviderUsage};
    use goose_providers::errors::ProviderError;
    use rmcp::model::Tool;
    use tempfile::TempDir;
    use test_case::test_case;

    const NUM_CONCURRENT_SESSIONS: i32 = 10;
    const GENERATED_SESSION_NAME: &str = "Generated session name";

    #[cfg(unix)]
    #[tokio::test]
    async fn session_directory_is_owner_private_for_fresh_and_existing_database() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let session_dir = temp_dir.path().join(SESSIONS_FOLDER);
        let database_path = session_dir.join(DB_NAME);

        let storage = SessionStorage::new(temp_dir.path().to_path_buf());
        storage.pool().await.unwrap();
        assert_eq!(
            fs::metadata(&session_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        storage.pool.close().await;
        drop(storage);

        fs::set_permissions(&database_path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&session_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            fs::metadata(&database_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(&session_dir).unwrap().permissions().mode() & 0o777,
            0o755
        );

        let session_manager = SessionManager::new(temp_dir.path().to_path_buf());
        let session = session_manager
            .create_session(
                PathBuf::from("/tmp/private-session-store"),
                "Private session".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();
        let loaded = session_manager
            .get_session(&session.id, false)
            .await
            .unwrap();

        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.name, "Private session");
        assert_eq!(
            fs::metadata(&session_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn azure_session_model_config_preserves_suffixed_deployment_id() {
        let json = serde_json::to_string(&ModelConfig {
            model_name: "gpt-5-high".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        })
        .unwrap();

        let config = deserialize_session_model_config(
            Some(goose_providers::azure_foundry::AZURE_FOUNDRY_PROVIDER_NAME),
            &json,
        )
        .unwrap();

        assert_eq!(config.model_name, "gpt-5-high");
        assert_eq!(config.thinking_effort(), None);
        assert!(!json.contains("context_limit"));
    }

    #[test]
    fn legacy_session_context_limit_is_ignored() {
        let config = deserialize_session_model_config(
            Some("openai"),
            r#"{"model_name":"gpt-4o","context_limit":64000,"temperature":null,"max_tokens":null,"toolshim":false,"toolshim_model":null}"#,
        )
        .unwrap();

        assert_eq!(config.context_limit, None);
    }

    #[test]
    fn azure_session_model_config_preserves_explicit_thinking_effort() {
        let config = deserialize_session_model_config(
            Some(goose_providers::azure_foundry::AZURE_FOUNDRY_PROVIDER_NAME),
            r#"{"model_name":"gpt-5-high","context_limit":null,"temperature":null,"max_tokens":null,"toolshim":false,"toolshim_model":null,"request_params":{"thinking_effort":"low"}}"#,
        )
        .unwrap();

        assert_eq!(config.model_name, "gpt-5-high");
        assert_eq!(
            config.thinking_effort(),
            Some(goose_providers::thinking::ThinkingEffort::Low)
        );
    }

    #[test]
    fn non_azure_session_model_config_keeps_suffix_normalization() {
        let config = deserialize_session_model_config(
            Some(goose_providers::openai::OPEN_AI_PROVIDER_NAME),
            r#"{"model_name":"gpt-5-high","context_limit":null,"temperature":null,"max_tokens":null,"toolshim":false,"toolshim_model":null}"#,
        )
        .unwrap();

        assert_eq!(config.model_name, "gpt-5");
        assert_eq!(
            config.thinking_effort(),
            Some(goose_providers::thinking::ThinkingEffort::High)
        );
    }

    struct NamingTestProvider;

    struct StatefulNamingTestProvider;

    struct LocalNamingTestProvider;

    #[async_trait::async_trait]
    impl Provider for NamingTestProvider {
        fn get_name(&self) -> &str {
            "naming-test"
        }

        async fn stream(
            &self,
            _model_config: &ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[rmcp::model::Tool],
        ) -> std::result::Result<MessageStream, goose_providers::errors::ProviderError> {
            unimplemented!("session naming calls complete")
        }

        async fn complete(
            &self,
            _model_config: &ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[Tool],
        ) -> Result<(Message, ProviderUsage), ProviderError> {
            Ok((
                Message::assistant().with_text(GENERATED_SESSION_NAME),
                ProviderUsage::new("test".to_string(), Default::default()),
            ))
        }
    }

    #[async_trait::async_trait]
    impl Provider for StatefulNamingTestProvider {
        fn get_name(&self) -> &str {
            "stateful-naming-test"
        }

        async fn stream(
            &self,
            _model_config: &ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[Tool],
        ) -> Result<MessageStream, ProviderError> {
            panic!("stateful session naming must not call the provider")
        }

        fn manages_own_context(&self) -> bool {
            true
        }
    }

    #[async_trait::async_trait]
    impl Provider for LocalNamingTestProvider {
        fn get_name(&self) -> &str {
            "local-naming-test"
        }

        async fn stream(
            &self,
            _model_config: &ModelConfig,
            _system: &str,
            _messages: &[Message],
            _tools: &[Tool],
        ) -> Result<MessageStream, ProviderError> {
            panic!("local session naming must not call the provider")
        }

        fn uses_local_session_naming(&self) -> bool {
            true
        }
    }

    fn naming_test_provider() -> Arc<dyn Provider> {
        Arc::new(NamingTestProvider)
    }

    fn test_recipe(title: &str) -> Recipe {
        Recipe::builder()
            .title(title)
            .description("Recipe description")
            .instructions("Follow the recipe")
            .build()
            .unwrap()
    }

    async fn create_session_for_list(
        sm: &SessionManager,
        working_dir: &str,
        has_message: bool,
    ) -> String {
        let session = sm
            .create_session(
                PathBuf::from(working_dir),
                format!("Session in {working_dir}"),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        if has_message {
            sm.add_message(&session.id, &Message::user().with_text("message"))
                .await
                .unwrap();
        }

        session.id
    }

    async fn create_session_for_list_with_message(
        sm: &SessionManager,
        working_dir: &str,
        message: &str,
    ) -> String {
        let session_id = create_session_for_list(sm, working_dir, false).await;
        sm.add_message(&session_id, &Message::user().with_text(message))
            .await
            .unwrap();
        session_id
    }

    async fn set_sessions_updated_at(
        sm: &SessionManager,
        session_ids: &[String],
        updated_at: &str,
    ) {
        let pool = sm.storage().pool().await.unwrap();
        let updated_at = chrono::DateTime::parse_from_rfc3339(updated_at).unwrap();
        let timestamp = updated_at.format("%Y-%m-%d %H:%M:%S").to_string();

        for session_id in session_ids {
            sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
                .bind(&timestamp)
                .bind(session_id)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    async fn add_message_at(sm: &SessionManager, session_id: &str, text: &str, timestamp: &str) {
        sm.add_message(session_id, &Message::user().with_text(text))
            .await
            .unwrap();

        let pool = sm.storage().pool().await.unwrap();
        let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        let timestamp_string = timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

        sqlx::query(
            "UPDATE messages SET timestamp = ?, created_timestamp = ? WHERE id = (SELECT MAX(id) FROM messages WHERE session_id = ?)",
        )
        .bind(&timestamp_string)
        .bind(timestamp.timestamp())
        .bind(session_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_message_at_millis(
        sm: &SessionManager,
        session_id: &str,
        text: &str,
        timestamp: &str,
    ) {
        sm.add_message(session_id, &Message::user().with_text(text))
            .await
            .unwrap();

        let pool = sm.storage().pool().await.unwrap();
        let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        let timestamp_string = timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

        sqlx::query(
            "UPDATE messages SET timestamp = ?, created_timestamp = ? WHERE id = (SELECT MAX(id) FROM messages WHERE session_id = ?)",
        )
        .bind(&timestamp_string)
        .bind(timestamp.timestamp_millis())
        .bind(session_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn set_message_timestamp(
        sm: &SessionManager,
        session_id: &str,
        message_id: &str,
        timestamp: &str,
    ) {
        let pool = sm.storage().pool().await.unwrap();
        let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        let timestamp_string = timestamp.format("%Y-%m-%d %H:%M:%S").to_string();

        sqlx::query(
            "UPDATE messages SET timestamp = ?, created_timestamp = ? WHERE session_id = ? AND message_id = ?",
        )
        .bind(&timestamp_string)
        .bind(timestamp.timestamp())
        .bind(session_id)
        .bind(message_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_user_message(sm: &SessionManager, session_id: &str) {
        sm.add_message(session_id, &Message::user().with_text("hello world"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn messages_read_back_in_the_order_they_arrived() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                "Arrival order".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        // A message built well before the one it is appended after: operations do
        // this whenever they prepare a reply and fill it in while a tool runs.
        let mut held = Message::user().with_text("built first");
        held.created -= 5;
        sm.add_message(&session.id, &Message::user().with_text("appended first"))
            .await
            .unwrap();
        sm.add_message(&session.id, &held).await.unwrap();

        let conversation = sm
            .get_session(&session.id, true)
            .await
            .unwrap()
            .conversation
            .expect("session has a conversation");
        let texts: Vec<_> = conversation
            .messages()
            .iter()
            .map(Message::as_concat_text)
            .collect();
        assert_eq!(texts, ["appended first", "built first"]);
    }

    #[tokio::test]
    async fn test_messages_session_created_index_avoids_disk_sort() {
        use sqlx::Row;

        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let pool = sm.storage.pool().await.unwrap();

        let index_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_messages_session_created')",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert!(
            index_exists,
            "idx_messages_session_created should exist after schema init"
        );

        let plan_rows = sqlx::query(
            "EXPLAIN QUERY PLAN \
             SELECT content_json FROM messages WHERE session_id = ? ORDER BY created_timestamp, id",
        )
        .bind("nonexistent_session")
        .fetch_all(pool)
        .await
        .unwrap();
        let plan_text: String = plan_rows
            .iter()
            .map(|r| r.try_get::<String, _>("detail").unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            plan_text.contains("idx_messages_session_created"),
            "loading a session's messages should use idx_messages_session_created, got: {plan_text}"
        );
        assert!(
            !plan_text.contains("TEMP B-TREE"),
            "loading a session's messages must not require an on-disk sort, got: {plan_text}"
        );
    }

    #[tokio::test]
    async fn test_last_message_at_is_derived_from_messages() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                "Session recency".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        let empty = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(empty.message_count, 0);
        assert_eq!(empty.last_message_at, None);

        add_message_at_millis(&sm, &session.id, "older", "2026-01-01T00:00:00Z").await;
        add_message_at(&sm, &session.id, "newer", "2026-01-02T03:04:05Z").await;

        let expected = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);

        let without_messages = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(without_messages.message_count, 2);
        assert_eq!(without_messages.last_message_at, Some(expected));

        let with_messages = sm.get_session(&session.id, true).await.unwrap();
        assert_eq!(with_messages.message_count, 2);
        assert_eq!(with_messages.last_message_at, Some(expected));
    }

    #[tokio::test]
    async fn test_message_count_excludes_agent_only_messages() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                "Hidden events".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        add_user_message(&sm, &session.id).await;
        sm.add_message(
            &session.id,
            &Message::user()
                .with_text("<turn-context>frozen</turn-context>")
                .with_visibility(false, true),
        )
        .await
        .unwrap();
        sm.add_message(&session.id, &Message::assistant().with_text("hi"))
            .await
            .unwrap();

        let counted = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(counted.message_count, 2);

        let with_messages = sm.get_session(&session.id, true).await.unwrap();
        assert_eq!(with_messages.message_count, 2);

        let listed = sm.list_sessions().await.unwrap();
        let listed_session = listed.iter().find(|s| s.id == session.id).unwrap();
        assert_eq!(listed_session.message_count, 2);

        let limited = sm.list_sessions_with_limit(1).await.unwrap();
        assert_eq!(limited[0].id, session.id);
        assert_eq!(limited[0].message_count, 2);
    }

    #[tokio::test]
    async fn test_truncate_conversation_from_message_keeps_same_second_previous_rows() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "Same second truncation".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        let timestamp = "2026-06-23T12:00:00Z";
        sm.add_message(
            &session.id,
            &Message::assistant()
                .with_text("assistant reply")
                .with_id("assistant"),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &session.id, "assistant", timestamp).await;

        sm.add_message(
            &session.id,
            &Message::user()
                .with_text("terminal history")
                .with_id("terminal-history"),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &session.id, "terminal-history", timestamp).await;

        sm.add_message(
            &session.id,
            &Message::user()
                .with_text("next prompt")
                .with_id("next-prompt"),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &session.id, "next-prompt", timestamp).await;

        sm.truncate_conversation_from_message(&session.id, "terminal-history")
            .await
            .unwrap();

        let reloaded = sm.get_session(&session.id, true).await.unwrap();
        let messages = reloaded.conversation.unwrap().messages().to_vec();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id.as_deref(), Some("assistant"));
        assert_eq!(messages[0].as_concat_text(), "assistant reply");
    }

    #[tokio::test]
    async fn test_maybe_update_name_updates_eligible_session() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "New Chat".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();

        add_user_message(&sm, &session.id).await;

        let update = sm
            .maybe_update_name(&session.id, naming_test_provider())
            .await
            .unwrap();
        assert_eq!(
            update.as_ref().map(|update| update.name.as_str()),
            Some(GENERATED_SESSION_NAME)
        );

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.name, GENERATED_SESSION_NAME);
        assert!(!reloaded.user_set_name);
    }

    #[tokio::test]
    async fn test_maybe_update_name_uses_local_name_for_stateful_provider() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "New Chat".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();
        sm.add_message(
            &session.id,
            &Message::user().with_text("investigate session naming with ACP providers"),
        )
        .await
        .unwrap();

        let update = sm
            .maybe_update_name(&session.id, Arc::new(StatefulNamingTestProvider))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(update.name, "investigate session naming with");
    }

    #[tokio::test]
    async fn test_maybe_update_name_uses_local_name_for_stateless_provider() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "New Chat".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .model_config(ModelConfig::new("test-model"))
            .apply()
            .await
            .unwrap();
        sm.add_message(
            &session.id,
            &Message::user().with_text("investigate local naming without provider completion"),
        )
        .await
        .unwrap();

        let update = sm
            .maybe_update_name(&session.id, Arc::new(LocalNamingTestProvider))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(update.name, "investigate local naming without");
    }

    #[tokio::test]
    async fn test_maybe_update_name_preserves_user_renamed_session() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "New Chat".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .user_provided_name("Manual title".to_string())
            .apply()
            .await
            .unwrap();
        add_user_message(&sm, &session.id).await;

        let update = sm
            .maybe_update_name(&session.id, naming_test_provider())
            .await
            .unwrap();
        assert!(update.is_none());

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.name, "Manual title");
        assert!(reloaded.user_set_name);
    }

    #[tokio::test]
    async fn test_maybe_update_name_uses_recipe_title_for_recipe_session() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "New Chat".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .recipe(Some(test_recipe("Recipe title")))
            .apply()
            .await
            .unwrap();
        add_user_message(&sm, &session.id).await;

        let update = sm
            .maybe_update_name(&session.id, naming_test_provider())
            .await
            .unwrap();
        assert_eq!(
            update.as_ref().map(|update| update.name.as_str()),
            Some("Recipe title")
        );

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.name, "Recipe title");
        assert!(!reloaded.user_set_name);

        let update = sm
            .maybe_update_name(&session.id, naming_test_provider())
            .await
            .unwrap();
        assert!(update.is_none());
    }

    #[tokio::test]
    async fn test_maybe_update_name_preserves_scheduled_session() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let original_name = "Scheduled job: test-job";

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                original_name.to_string(),
                SessionType::Scheduled,
                GooseMode::default(),
            )
            .await
            .unwrap();

        add_user_message(&sm, &session.id).await;

        let update = sm
            .maybe_update_name(&session.id, naming_test_provider())
            .await
            .unwrap();
        assert!(update.is_none());

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.name, original_name);
        assert!(!reloaded.user_set_name);
    }

    async fn create_search_session(
        sm: &SessionManager,
        name: &str,
        session_type: SessionType,
        updated_at: &str,
        messages: &[(&str, &str)],
    ) -> String {
        let session = sm
            .create_session(
                PathBuf::from("/tmp/search-test"),
                name.to_string(),
                session_type,
                GooseMode::default(),
            )
            .await
            .unwrap();

        for (text, timestamp) in messages {
            add_message_at(sm, &session.id, text, timestamp).await;
        }
        set_sessions_updated_at(sm, std::slice::from_ref(&session.id), updated_at).await;

        session.id
    }

    #[tokio::test]
    async fn test_search_chat_history_preserves_message_limited_behavior() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let _older_target = create_search_session(
            &sm,
            "Older target",
            SessionType::User,
            "2026-05-01T00:00:00Z",
            &[(
                "does Acme have an email address for John Doe",
                "2026-05-01T00:00:00Z",
            )],
        )
        .await;

        let newer_noise = create_search_session(
            &sm,
            "Newer noise",
            SessionType::User,
            "2026-05-22T00:00:00Z",
            &[
                ("Acme person name looking for Acme", "2026-05-22T00:00:00Z"),
                (
                    "another Acme person name looking for Acme",
                    "2026-05-22T00:01:00Z",
                ),
            ],
        )
        .await;

        let results = sm
            .search_chat_history("Acme", Some(2), None, None, None, vec![SessionType::User])
            .await
            .unwrap();

        assert_eq!(results.results.len(), 1);
        assert_eq!(results.results[0].session_id, newer_noise);
        assert_eq!(results.results[0].messages.len(), 2);
    }

    async fn expected_session_list_ids(sm: &SessionManager, session_ids: &[String]) -> Vec<String> {
        let mut sessions = Vec::new();
        for session_id in session_ids {
            sessions.push(sm.get_session(session_id, false).await.unwrap());
        }
        sessions.sort_by(|a, b| {
            session_sort_at(b)
                .cmp(&session_sort_at(a))
                .then_with(|| b.id.cmp(&a.id))
        });
        sessions.into_iter().map(|session| session.id).collect()
    }

    async fn assert_session_list_page(
        sm: &SessionManager,
        cursor: Option<&SessionListCursor>,
        working_dir: Option<&str>,
        page_size: usize,
        expected_ids: &[String],
        expected_next_cursor: bool,
    ) -> Option<SessionListCursor> {
        let types = [SessionType::User];
        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    working_dir: working_dir.map(Path::new),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor,
                page_size,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids.as_slice(), expected_ids);
        assert_eq!(page.next_cursor.is_some(), expected_next_cursor);
        page.next_cursor
    }

    async fn run_lock_upgrade_attempt(
        pool: Pool<Sqlite>,
        session_id: String,
        begin_statement: &'static str,
        worker_id: i32,
        barrier: Option<Arc<tokio::sync::Barrier>>,
    ) -> anyhow::Result<()> {
        let mut tx = pool.begin_with(begin_statement).await?;

        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_one(&mut *tx)
            .await?;

        if let Some(barrier) = barrier {
            barrier.wait().await;
        }

        sqlx::query("UPDATE sessions SET total_tokens = ? WHERE id = ?")
            .bind(worker_id)
            .bind(&session_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn run_lock_upgrade_race(
        pool: Pool<Sqlite>,
        session_id: String,
        begin_statement: &'static str,
        use_barrier: bool,
    ) -> Vec<anyhow::Result<()>> {
        let barrier = if use_barrier {
            Some(Arc::new(tokio::sync::Barrier::new(2)))
        } else {
            None
        };
        let mut handles = Vec::new();

        for worker_id in 0..2 {
            let pool = pool.clone();
            let session_id = session_id.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                run_lock_upgrade_attempt(pool, session_id, begin_statement, worker_id, barrier)
                    .await
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.expect("lock-upgrade task panicked"));
        }
        results
    }

    #[tokio::test]
    async fn test_begin_immediate_prevents_lock_upgrade_deadlock() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = SessionManager::new(temp_dir.path().to_path_buf());

        let session = session_manager
            .create_session(
                PathBuf::from("/tmp/lock-upgrade-test"),
                "Lock Upgrade Session".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        let pool = session_manager.storage().pool.clone();

        let results = run_lock_upgrade_race(pool.clone(), session.id.clone(), "BEGIN", true).await;
        assert!(
            results.iter().any(Result::is_err),
            "BEGIN (DEFERRED) should cause SQLITE_BUSY when two tasks try to upgrade SHARED → RESERVED"
        );

        let results = run_lock_upgrade_race(pool, session.id, "BEGIN IMMEDIATE", false).await;
        assert!(
            results.iter().all(Result::is_ok),
            "BEGIN IMMEDIATE should serialize contention without SQLITE_BUSY: {:?}",
            results
                .iter()
                .filter_map(|r| r.as_ref().err().map(ToString::to_string))
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn test_session_list_paged_first_second_and_final_page() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let mut expected_ids = Vec::new();
        for _ in 0..5 {
            expected_ids.push(create_session_for_list(&sm, "/tmp/session-list", true).await);
        }
        let expected_ids = expected_session_list_ids(&sm, &expected_ids).await;

        let cursor = assert_session_list_page(&sm, None, None, 2, &expected_ids[0..2], true).await;
        let cursor =
            assert_session_list_page(&sm, cursor.as_ref(), None, 2, &expected_ids[2..4], true)
                .await;
        assert_session_list_page(&sm, cursor.as_ref(), None, 2, &expected_ids[4..5], false).await;
    }

    #[tokio::test]
    async fn test_list_sessions_with_limit_counts_visible_and_legacy_messages() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let older = create_session_for_list(&sm, "/tmp/session-list", false).await;
        add_message_at(&sm, &older, "older", "2026-01-01T00:00:00Z").await;

        let selected = create_session_for_list(&sm, "/tmp/session-list", false).await;
        sm.add_message(
            &selected,
            &Message::user().with_id("legacy").with_text("legacy"),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &selected, "legacy", "2026-01-02T00:00:00Z").await;
        sqlx::query(
            "UPDATE messages SET metadata_json = '{}' WHERE session_id = ? AND message_id = ?",
        )
        .bind(&selected)
        .bind("legacy")
        .execute(sm.storage().pool().await.unwrap())
        .await
        .unwrap();

        sm.add_message(
            &selected,
            &Message::user()
                .with_id("hidden")
                .with_text("hidden")
                .with_visibility(false, true),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &selected, "hidden", "2026-01-03T00:00:00Z").await;

        sm.add_message(
            &selected,
            &Message::assistant().with_id("visible").with_text("visible"),
        )
        .await
        .unwrap();
        set_message_timestamp(&sm, &selected, "visible", "2026-01-04T00:00:00Z").await;

        let sessions = sm.list_sessions_with_limit(1).await.unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, selected);
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(
            sessions[0].last_message_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-01-04T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[tokio::test]
    async fn test_session_list_paged_includes_hidden_only_session_with_zero_count() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let hidden_only = create_session_for_list(&sm, "/tmp/session-list", false).await;
        sm.add_message(
            &hidden_only,
            &Message::user()
                .with_text("hidden")
                .with_visibility(false, true),
        )
        .await
        .unwrap();
        create_session_for_list(&sm, "/tmp/session-list", false).await;

        let types = [SessionType::User];
        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();

        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].id, hidden_only);
        assert_eq!(page.sessions[0].message_count, 0);
        assert!(page.sessions[0].last_message_at.is_some());
    }

    #[tokio::test]
    async fn test_list_sessions_with_limit_returns_empty_session_with_zero_count() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let empty = create_session_for_list(&sm, "/tmp/session-list", false).await;

        let sessions = sm.list_sessions_with_limit(1).await.unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, empty);
        assert_eq!(sessions[0].message_count, 0);
        assert_eq!(sessions[0].last_message_at, None);
    }

    #[tokio::test]
    async fn test_session_list_paged_sorts_by_last_message_at() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let stale_but_modified = create_session_for_list(&sm, "/tmp/session-list", false).await;
        add_message_at(
            &sm,
            &stale_but_modified,
            "older message",
            "2026-01-01T00:00:00Z",
        )
        .await;
        set_sessions_updated_at(
            &sm,
            std::slice::from_ref(&stale_but_modified),
            "2026-02-01T00:00:00Z",
        )
        .await;

        let active_but_not_modified =
            create_session_for_list(&sm, "/tmp/session-list", false).await;
        add_message_at(
            &sm,
            &active_but_not_modified,
            "newer message",
            "2026-01-02T00:00:00Z",
        )
        .await;
        set_sessions_updated_at(
            &sm,
            std::slice::from_ref(&active_but_not_modified),
            "2026-01-15T00:00:00Z",
        )
        .await;

        assert_session_list_page(
            &sm,
            None,
            None,
            2,
            &[active_but_not_modified, stale_but_modified],
            false,
        )
        .await;
    }

    #[tokio::test]
    async fn test_session_list_paged_uses_id_tiebreaker_for_duplicate_activity_time() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let mut expected_ids = Vec::new();
        for _ in 0..3 {
            expected_ids.push(create_session_for_list(&sm, "/tmp/session-list", true).await);
        }
        set_sessions_updated_at(&sm, &expected_ids, "2024-01-01T00:00:00Z").await;
        let expected_ids = expected_session_list_ids(&sm, &expected_ids).await;

        let cursor = assert_session_list_page(&sm, None, None, 2, &expected_ids[0..2], true).await;
        assert_session_list_page(&sm, cursor.as_ref(), None, 2, &expected_ids[2..3], false).await;
    }

    #[tokio::test]
    async fn test_session_list_paged_filters_empty_and_cwd_before_pagination() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let expected_ids = vec![
            create_session_for_list(&sm, "/tmp/session-list/a", true).await,
            create_session_for_list(&sm, "/tmp/session-list/a", true).await,
        ];
        create_session_for_list(&sm, "/tmp/session-list/a", false).await;
        create_session_for_list(&sm, "/tmp/session-list/b", true).await;
        let expected_ids = expected_session_list_ids(&sm, &expected_ids).await;

        let cursor = assert_session_list_page(
            &sm,
            None,
            Some("/tmp/session-list/a"),
            1,
            &expected_ids[0..1],
            true,
        )
        .await;
        assert_session_list_page(
            &sm,
            cursor.as_ref(),
            Some("/tmp/session-list/a"),
            1,
            &expected_ids[1..2],
            false,
        )
        .await;
    }

    #[tokio::test]
    async fn test_session_list_paged_filters_by_keyword() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let target = create_session_for_list_with_message(
            &sm,
            "/tmp/session-list",
            "Discuss Postgres migrations",
        )
        .await;
        create_session_for_list_with_message(&sm, "/tmp/session-list", "Plan the mobile release")
            .await;

        let types = [SessionType::User];
        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    keyword: Some("postgres"),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![target]);
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn test_session_list_paged_keyword_uses_or_terms() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let postgres = create_session_for_list_with_message(
            &sm,
            "/tmp/session-list",
            "Postgres migration plan",
        )
        .await;
        let sqlite =
            create_session_for_list_with_message(&sm, "/tmp/session-list", "SQLite backup notes")
                .await;
        create_session_for_list_with_message(&sm, "/tmp/session-list", "Mobile release notes")
            .await;
        let expected_ids = expected_session_list_ids(&sm, &[postgres, sqlite]).await;

        let types = [SessionType::User];
        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    keyword: Some("postgres sqlite"),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();

        assert_eq!(ids, expected_ids);
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn test_session_list_paged_empty_keyword_matches_plain_list() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let expected_ids = vec![
            create_session_for_list_with_message(&sm, "/tmp/session-list", "first message").await,
            create_session_for_list_with_message(&sm, "/tmp/session-list", "second message").await,
        ];
        let expected_ids = expected_session_list_ids(&sm, &expected_ids).await;

        let types = [SessionType::User];
        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    keyword: Some("   "),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();

        assert_eq!(ids, expected_ids);
    }

    #[tokio::test]
    async fn test_session_list_paged_keyword_treats_like_wildcards_as_literals() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let percent_id =
            create_session_for_list_with_message(&sm, "/tmp/session-list", "Deploy is 100% done")
                .await;
        let underscore_id = create_session_for_list_with_message(
            &sm,
            "/tmp/session-list",
            "feature_flag is enabled",
        )
        .await;
        create_session_for_list_with_message(&sm, "/tmp/session-list", "plain message").await;

        let types = [SessionType::User];
        let percent_page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    keyword: Some("%"),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let percent_ids = percent_page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(percent_ids, vec![percent_id]);

        let underscore_page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: SessionListFilters {
                    types: Some(&types),
                    keyword: Some("_"),
                    only_sessions_with_messages: true,
                    ..Default::default()
                },
                cursor: None,
                page_size: 10,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let underscore_ids = underscore_page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(underscore_ids, vec![underscore_id]);
    }

    #[tokio::test]
    async fn test_session_list_paged_keyword_combines_with_cwd_and_pagination() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let expected_ids = vec![
            create_session_for_list_with_message(&sm, "/tmp/session-list/a", "Postgres plan one")
                .await,
            create_session_for_list_with_message(&sm, "/tmp/session-list/a", "Postgres plan two")
                .await,
        ];
        create_session_for_list_with_message(&sm, "/tmp/session-list/a", "Mobile release").await;
        create_session_for_list_with_message(&sm, "/tmp/session-list/b", "Postgres plan other")
            .await;
        let expected_ids = expected_session_list_ids(&sm, &expected_ids).await;

        let types = [SessionType::User];
        let filters = SessionListFilters {
            types: Some(&types),
            working_dir: Some(Path::new("/tmp/session-list/a")),
            keyword: Some("postgres"),
            only_sessions_with_messages: true,
        };
        let cursor = sm
            .list_sessions_paged(SessionListPageQuery {
                filters: filters.clone(),
                cursor: None,
                page_size: 1,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = cursor
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected_ids[0..1]);
        assert!(cursor.next_cursor.is_some());

        let page = sm
            .list_sessions_paged(SessionListPageQuery {
                filters,
                cursor: cursor.next_cursor.as_ref(),
                page_size: 1,
                include_last_message_snippet: false,
            })
            .await
            .unwrap();
        let ids = page
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected_ids[1..2]);
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_session_creation() {
        let temp_dir = TempDir::new().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_dir.path().to_path_buf()));

        let mut handles = vec![];

        for i in 0..NUM_CONCURRENT_SESSIONS {
            let sm = Arc::clone(&session_manager);
            let handle = tokio::spawn(async move {
                let working_dir = PathBuf::from(format!("/tmp/test_{}", i));
                let description = format!("Test session {}", i);

                let session = sm
                    .create_session(
                        working_dir.clone(),
                        description,
                        SessionType::User,
                        GooseMode::default(),
                    )
                    .await
                    .unwrap();

                sm.add_message(
                    &session.id,
                    &Message {
                        id: None,
                        role: Role::User,
                        created: chrono::Utc::now().timestamp_millis(),
                        content: vec![MessageContent::text("hello world")],
                        metadata: Default::default(),
                    },
                )
                .await
                .unwrap();

                sm.add_message(
                    &session.id,
                    &Message {
                        id: None,
                        role: Role::Assistant,
                        created: chrono::Utc::now().timestamp_millis(),
                        content: vec![MessageContent::text("sup world?")],
                        metadata: Default::default(),
                    },
                )
                .await
                .unwrap();

                sm.update(&session.id)
                    .user_provided_name(format!("Updated session {}", i))
                    .usage(Usage::new(None, None, Some(100 * i)))
                    .apply()
                    .await
                    .unwrap();

                let updated = sm.get_session(&session.id, true).await.unwrap();
                assert_eq!(updated.message_count, 2);
                assert_eq!(updated.usage.total_tokens, Some(100 * i));

                session.id
            });
            handles.push(handle);
        }

        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        assert_eq!(results.len(), NUM_CONCURRENT_SESSIONS as usize);

        let unique_ids: std::collections::HashSet<_> = results.iter().collect();
        assert_eq!(unique_ids.len(), NUM_CONCURRENT_SESSIONS as usize);

        let sessions = session_manager.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), NUM_CONCURRENT_SESSIONS as usize);

        for session in &sessions {
            assert_eq!(session.message_count, 2);
            assert!(session.name.starts_with("Updated session"));
        }

        let insights = session_manager.get_insights().await.unwrap();
        assert_eq!(insights.total_sessions, NUM_CONCURRENT_SESSIONS as usize);
        let expected_tokens = 100 * NUM_CONCURRENT_SESSIONS * (NUM_CONCURRENT_SESSIONS - 1) / 2;
        assert_eq!(insights.total_tokens, expected_tokens as i64);
    }

    #[tokio::test]
    async fn test_export_import_roundtrip() {
        const DESCRIPTION: &str = "Original session";
        const USER_MESSAGE: &str = "test message";
        const ASSISTANT_MESSAGE: &str = "test response";

        let usage =
            Usage::new(Some(300), Some(200), Some(500)).with_cache_tokens(Some(120), Some(80));
        let accumulated_usage =
            Usage::new(Some(600), Some(400), Some(1000)).with_cache_tokens(Some(400), Some(150));

        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let original = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                DESCRIPTION.to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&original.id)
            .usage(usage)
            .accumulated_usage(accumulated_usage)
            .apply()
            .await
            .unwrap();

        sm.add_message(
            &original.id,
            &Message {
                id: None,
                role: Role::User,
                created: chrono::Utc::now().timestamp_millis(),
                content: vec![MessageContent::text(USER_MESSAGE)],
                metadata: Default::default(),
            },
        )
        .await
        .unwrap();

        sm.add_message(
            &original.id,
            &Message {
                id: None,
                role: Role::Assistant,
                created: chrono::Utc::now().timestamp_millis(),
                content: vec![MessageContent::text(ASSISTANT_MESSAGE)],
                metadata: Default::default(),
            },
        )
        .await
        .unwrap();

        let exported = sm.export_session(&original.id).await.unwrap();
        let imported = sm.import_session(&exported, None).await.unwrap();

        assert_ne!(imported.id, original.id);
        assert_eq!(imported.name, DESCRIPTION);
        assert_eq!(imported.working_dir, PathBuf::from("/tmp/test"));
        assert_eq!(imported.usage, usage);
        assert_eq!(imported.accumulated_usage, accumulated_usage);
        assert_eq!(imported.message_count, 2);

        let conversation = imported.conversation.unwrap();
        assert_eq!(conversation.messages().len(), 2);
        assert_eq!(conversation.messages()[0].role, Role::User);
        assert_eq!(conversation.messages()[1].role, Role::Assistant);
    }

    #[tokio::test]
    async fn test_list_sessions_filters_by_type() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let user_session = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                "User session".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.add_message(
            &user_session.id,
            &Message {
                id: None,
                role: Role::User,
                created: chrono::Utc::now().timestamp_millis(),
                content: vec![MessageContent::text("hello world")],
                metadata: Default::default(),
            },
        )
        .await
        .unwrap();

        let acp_session = sm
            .create_session(
                PathBuf::from("/tmp/test"),
                "ACP session".to_string(),
                SessionType::Acp,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.add_message(
            &acp_session.id,
            &Message {
                id: None,
                role: Role::User,
                created: chrono::Utc::now().timestamp_millis(),
                content: vec![MessageContent::text("hello acp")],
                metadata: Default::default(),
            },
        )
        .await
        .unwrap();

        let default_sessions = sm.list_sessions().await.unwrap();
        assert_eq!(default_sessions.len(), 1);
        assert_eq!(default_sessions[0].name, "User session");

        let acp_sessions = sm
            .list_sessions_by_types(&[SessionType::Acp])
            .await
            .unwrap();
        assert_eq!(acp_sessions.len(), 1);
        assert_eq!(acp_sessions[0].name, "ACP session");
    }

    #[tokio::test]
    async fn test_import_session_with_legacy_flat_fields() {
        const OLD_FORMAT_JSON: &str = r#"{
            "id": "20240101_1",
            "description": "Old format session",
            "user_set_name": true,
            "working_dir": "/tmp/test",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "extension_data": {},
            "message_count": 0,
            "total_tokens": 500,
            "input_tokens": 300,
            "output_tokens": 200,
            "cache_read_tokens": 120,
            "accumulated_total_tokens": 1000,
            "accumulated_input_tokens": 600,
            "accumulated_output_tokens": 400
        }"#;

        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let imported = sm.import_session(OLD_FORMAT_JSON, None).await.unwrap();

        assert_eq!(imported.name, "Old format session");
        assert!(imported.user_set_name);
        assert_eq!(imported.working_dir, PathBuf::from("/tmp/test"));
        assert_eq!(
            imported.usage,
            Usage::new(Some(300), Some(200), Some(500)).with_cache_tokens(Some(120), None)
        );
        assert_eq!(
            imported.accumulated_usage,
            Usage::new(Some(600), Some(400), Some(1000))
        );
    }

    #[test_case(GooseMode::Approve)]
    #[test_case(GooseMode::SmartApprove)]
    #[test_case(GooseMode::Chat)]
    #[tokio::test]
    async fn test_goose_mode_persists(mode: GooseMode) {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "test".into(),
                SessionType::User,
                mode,
            )
            .await
            .unwrap();

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.goose_mode, mode);
    }

    #[tokio::test]
    async fn test_goose_mode_update() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "test".into(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .unwrap();

        sm.update(&session.id)
            .goose_mode(GooseMode::Approve)
            .apply()
            .await
            .unwrap();

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.goose_mode, GooseMode::Approve);
    }

    #[tokio::test]
    async fn test_goose_mode_malformed_defaults_to_auto() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());

        let session = sm
            .create_session(
                temp_dir.path().to_path_buf(),
                "test".into(),
                SessionType::User,
                GooseMode::Approve,
            )
            .await
            .unwrap();

        let pool = &sm.storage().pool;
        sqlx::query("UPDATE sessions SET goose_mode = 'garbage' WHERE id = ?")
            .bind(&session.id)
            .execute(pool)
            .await
            .unwrap();

        let reloaded = sm.get_session(&session.id, false).await.unwrap();
        assert_eq!(reloaded.goose_mode, GooseMode::default());
    }

    #[tokio::test]
    async fn test_acp_session_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join(SESSIONS_FOLDER).join(DB_NAME);

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();

        SessionStorage::create_schema(&pool).await.unwrap();

        // Demote the schema back to v8 to simulate a database
        // that has never seen migration 9.
        sqlx::query("UPDATE schema_version SET version = 8")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, name, user_set_name, session_type, working_dir, extension_data, goose_mode)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("user_id")
        .bind("User Session")
        .bind(false)
        .bind("user")
        .bind("/tmp")
        .bind("{}")
        .bind("auto")
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, name, user_set_name, session_type, working_dir, extension_data, goose_mode)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("acp_id")
        .bind("ACP Session")
        .bind(false)
        .bind("user")
        .bind("/tmp")
        .bind("{}")
        .bind("auto")
        .execute(&pool)
        .await
        .unwrap();

        pool.close().await;

        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        sm.storage().pool().await.unwrap(); // Triggers migration

        let user_session = sm.storage().get_session("user_id", false).await.unwrap();
        assert_eq!(user_session.session_type, SessionType::User);

        let acp_session = sm.storage().get_session("acp_id", false).await.unwrap();
        assert_eq!(acp_session.session_type, SessionType::Acp);
    }

    #[tokio::test]
    async fn test_cache_token_columns_migration_and_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join(SESSIONS_FOLDER).join(DB_NAME);

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();

        SessionStorage::create_schema(&pool).await.unwrap();

        // Recreate a v13-shaped database without cache token columns.
        for column in [
            "cache_read_tokens",
            "cache_write_tokens",
            "accumulated_cache_read_tokens",
            "accumulated_cache_write_tokens",
        ] {
            sqlx::query(AssertSqlSafe(format!(
                "ALTER TABLE sessions DROP COLUMN {column}"
            )))
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE schema_version SET version = 13")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, name, user_set_name, session_type, working_dir, extension_data, goose_mode)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("cache_id")
        .bind("Cache Session")
        .bind(false)
        .bind("user")
        .bind("/tmp")
        .bind("{}")
        .bind("auto")
        .execute(&pool)
        .await
        .unwrap();

        pool.close().await;

        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        sm.storage().pool().await.unwrap(); // Triggers migration

        let usage =
            Usage::new(Some(8000), Some(500), None).with_cache_tokens(Some(5000), Some(1000));
        let accumulated_usage =
            Usage::new(Some(24000), Some(1500), None).with_cache_tokens(Some(15000), Some(3000));

        sm.update("cache_id")
            .usage(usage)
            .accumulated_usage(accumulated_usage)
            .apply()
            .await
            .unwrap();

        let loaded = sm.get_session("cache_id", false).await.unwrap();
        assert_eq!(loaded.usage, usage);
        assert_eq!(loaded.accumulated_usage, accumulated_usage);
    }

    fn message_usage(input: i32, output: i32, cost: f64, is_compaction: bool) -> MessageUsage {
        MessageUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            total_tokens: Some(input + output),
            cost: Some(cost),
            cost_source: Some(CostSource::Estimated),
            is_compaction,
            ..Default::default()
        }
    }

    async fn new_session(sm: &SessionManager) -> String {
        sm.create_session(
            PathBuf::from("/tmp"),
            "s".to_string(),
            SessionType::User,
            GooseMode::default(),
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_ledger(
        sm: &SessionManager,
        session_id: &str,
        usage: &MessageUsage,
    ) -> Result<()> {
        let pool = sm.storage().pool().await?;
        let mut tx = pool.begin().await?;
        insert_usage_ledger_row(&mut tx, session_id, None, usage).await?;
        tx.commit().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_usage_totals_include_subagent_tree() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let parent = new_session(&sm).await;
        let child = new_session(&sm).await;
        sm.update(&child)
            .parent_session_id(Some(parent.clone()))
            .apply()
            .await
            .unwrap();

        seed_ledger(&sm, &parent, &message_usage(100, 20, 0.10, false))
            .await
            .unwrap();
        seed_ledger(&sm, &child, &message_usage(40, 8, 0.04, false))
            .await
            .unwrap();

        let parent_totals = sm.get_session_usage_totals(&parent).await.unwrap();
        assert_eq!(parent_totals.accumulated_usage.input_tokens, Some(140));
        assert!((parent_totals.accumulated_cost.unwrap() - 0.14).abs() < 1e-9);

        let child_totals = sm.get_session_usage_totals(&child).await.unwrap();
        assert_eq!(child_totals.accumulated_usage.input_tokens, Some(40));
        assert!((child_totals.accumulated_cost.unwrap() - 0.04).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_ledger_reconciles_spend_recorded_on_pre_v15_builds() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        sm.update(&id)
            .accumulated_usage(Usage::new(Some(5000), Some(1000), Some(6000)))
            .accumulated_cost(Some(5.0))
            .apply()
            .await
            .unwrap();

        sm.record_usage_metrics(
            &id,
            None,
            Usage::new(Some(100), Some(20), Some(120)),
            "test-model",
            &message_usage(100, 20, 0.01, false),
        )
        .await
        .unwrap();

        let totals = sm.get_session_usage_totals(&id).await.unwrap();
        assert_eq!(totals.accumulated_usage.total_tokens, Some(6120));
        assert!((totals.accumulated_cost.unwrap() - 5.01).abs() < 1e-9);

        let session = sm.get_session(&id, false).await.unwrap();
        sm.update(&id)
            .accumulated_usage(
                session.accumulated_usage + Usage::new(Some(500), Some(50), Some(550)),
            )
            .accumulated_cost(Some(session.accumulated_cost.unwrap() + 0.50))
            .apply()
            .await
            .unwrap();

        sm.record_usage_metrics(
            &id,
            None,
            Usage::new(Some(30), Some(5), Some(35)),
            "test-model",
            &message_usage(30, 5, 0.03, false),
        )
        .await
        .unwrap();

        let totals = sm.get_session_usage_totals(&id).await.unwrap();
        assert_eq!(totals.accumulated_usage.input_tokens, Some(5630));
        assert_eq!(totals.accumulated_usage.output_tokens, Some(1075));
        assert_eq!(totals.accumulated_usage.total_tokens, Some(6705));
        assert!((totals.accumulated_cost.unwrap() - 5.54).abs() < 1e-9);

        let session = sm.get_session(&id, false).await.unwrap();
        assert_eq!(session.accumulated_usage, totals.accumulated_usage);
        assert!((session.accumulated_cost.unwrap() - 5.54).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_usage_totals_read_through_unreconciled_drift() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        sm.record_usage_metrics(
            &id,
            None,
            Usage::new(Some(100), Some(20), Some(120)),
            "test-model",
            &message_usage(100, 20, 0.10, false),
        )
        .await
        .unwrap();

        let session = sm.get_session(&id, false).await.unwrap();
        sm.update(&id)
            .accumulated_usage(
                session.accumulated_usage + Usage::new(Some(500), Some(50), Some(550)),
            )
            .accumulated_cost(Some(session.accumulated_cost.unwrap() + 0.50))
            .apply()
            .await
            .unwrap();

        let totals = sm.get_session_usage_totals(&id).await.unwrap();
        assert_eq!(totals.accumulated_usage.input_tokens, Some(600));
        assert_eq!(totals.accumulated_usage.total_tokens, Some(670));
        assert!((totals.accumulated_cost.unwrap() - 0.60).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_usage_totals_fall_back_to_accumulated_for_legacy_sessions() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        sm.update(&id)
            .accumulated_usage(Usage::new(Some(500), Some(100), Some(600)))
            .accumulated_cost(Some(0.42))
            .apply()
            .await
            .unwrap();

        let totals = sm.get_session_usage_totals(&id).await.unwrap();
        assert_eq!(totals.accumulated_usage.input_tokens, Some(500));
        assert_eq!(totals.accumulated_cost, Some(0.42));
    }

    #[tokio::test]
    async fn test_usage_ledger_survives_conversation_replace() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        seed_ledger(&sm, &id, &message_usage(1000, 200, 1.0, false))
            .await
            .unwrap();
        seed_ledger(&sm, &id, &message_usage(50, 10, 0.05, true))
            .await
            .unwrap();

        sm.replace_conversation(&id, &Conversation::default())
            .await
            .unwrap();

        let totals = sm.get_session_usage_totals(&id).await.unwrap();
        assert_eq!(totals.accumulated_usage.total_tokens, Some(1260));
        assert!((totals.accumulated_cost.unwrap() - 1.05).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_usage_totals_mixed_legacy_and_ledger_tree() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let parent = new_session(&sm).await;
        let child = new_session(&sm).await;
        sm.update(&child)
            .parent_session_id(Some(parent.clone()))
            .apply()
            .await
            .unwrap();

        seed_ledger(&sm, &parent, &message_usage(100, 20, 0.10, false))
            .await
            .unwrap();
        sm.update(&child)
            .accumulated_usage(Usage::new(Some(300), Some(60), Some(360)))
            .accumulated_cost(Some(0.25))
            .apply()
            .await
            .unwrap();

        let totals = sm.get_session_usage_totals(&parent).await.unwrap();
        assert_eq!(totals.accumulated_usage.input_tokens, Some(400));
        assert_eq!(totals.accumulated_usage.output_tokens, Some(80));
        assert!((totals.accumulated_cost.unwrap() - 0.35).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_delete_session_with_ledger_rows() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        seed_ledger(&sm, &id, &message_usage(100, 20, 0.10, false))
            .await
            .unwrap();

        sm.delete_session(&id).await.unwrap();
        assert!(sm.get_session(&id, false).await.is_err());
    }

    #[tokio::test]
    async fn test_pre_v15_delete_cascades_ledger_rows() {
        let temp_dir = TempDir::new().unwrap();
        let sm = SessionManager::new(temp_dir.path().to_path_buf());
        let id = new_session(&sm).await;

        seed_ledger(&sm, &id, &message_usage(100, 20, 0.10, false))
            .await
            .unwrap();

        let pool = sm.storage().pool().await.unwrap();
        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(&id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id)
            .execute(pool)
            .await
            .unwrap();

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM usage_ledger WHERE session_id = ?")
                .bind(&id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0);
    }
}
