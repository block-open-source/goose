use agent_client_protocol::schema::v1::{AvailableCommand, ContentBlock, McpServer, SessionInfo};
use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod recipe;
pub use recipe::*;
mod schedule;
pub use schedule::*;

/// Schema descriptor for a single custom method, produced by the
/// `#[custom_methods]` macro's generated `custom_method_schemas()` function.
///
/// `params_schema` / `response_schema` hold `$ref` pointers or inline schemas
/// produced by `SchemaGenerator::subschema_for`. All referenced types are
/// collected in the generator's `$defs` map.
///
/// `params_type_name` / `response_type_name` carry the Rust struct name so the
/// binary can key `$defs` entries and annotate them with `x-method` / `x-side`.
#[derive(Debug, Serialize)]
pub struct CustomMethodSchema {
    pub method: String,
    pub params_schema: Option<schemars::Schema>,
    pub params_type_name: Option<String>,
    pub response_schema: Option<schemars::Schema>,
    pub response_type_name: Option<String>,
}

/// Add an extension to an active session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/extensions/add", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct AddSessionExtensionRequest {
    pub session_id: String,
    pub extension: GooseExtension,
}

/// Remove an extension from an active session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/extensions/remove", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct RemoveSessionExtensionRequest {
    pub session_id: String,
    pub extension_key: String,
}

/// List all tools available in a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/tools/list", response = GetToolsResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetToolsRequest {
    pub session_id: String,
    /// Filter tools to those belonging to this extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_name: Option<String>,
}

/// A single tool item returned by the tools list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolListItem {
    pub name: String,
    pub description: String,
    pub parameters: Vec<String>,
    pub permission: Option<ToolPermissionLevel>,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

/// Tools response.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct GetToolsResponse {
    pub tools: Vec<ToolListItem>,
}

/// Read a resource from an extension.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/resources/read", response = ReadResourceResponse)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceRequest {
    pub session_id: String,
    pub uri: String,
    pub extension_name: String,
}

/// Resource read response.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct ReadResourceResponse {
    /// The resource result from the extension (MCP ReadResourceResult).
    #[serde(default)]
    pub result: serde_json::Value,
}

/// Call a tool from an extension.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/tools/call", response = GooseToolCallResponse)]
#[serde(rename_all = "camelCase")]
pub struct GooseToolCallRequest {
    pub session_id: String,
    pub extension_name: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Tool call response.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GooseToolCallResponse {
    #[serde(default)]
    pub content: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<serde_json::Value>,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "_meta")]
    pub meta: Option<serde_json::Value>,
}

/// List available goose apps, optionally scoped to a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/apps/list", response = AppsListResponse)]
#[serde(rename_all = "camelCase")]
pub struct AppsListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct AppsListResponse {
    #[serde(default)]
    pub apps: Vec<serde_json::Value>,
}

/// Export a goose app as HTML.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/apps/export", response = AppsExportResponse)]
#[serde(rename_all = "camelCase")]
pub struct AppsExportRequest {
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct AppsExportResponse {
    pub html: String,
}

/// Import a goose app from HTML.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/apps/import", response = AppsImportResponse)]
#[serde(rename_all = "camelCase")]
pub struct AppsImportRequest {
    pub html: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct AppsImportResponse {
    pub name: String,
    pub message: String,
}

/// Delete a goose app by name.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/apps/delete", response = AppsDeleteResponse)]
#[serde(rename_all = "camelCase")]
pub struct AppsDeleteRequest {
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct AppsDeleteResponse {
    pub name: String,
    pub message: String,
}

/// Update the working directory for a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/working-dir/update", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkingDirRequest {
    pub session_id: String,
    pub working_dir: String,
}

/// How a session system prompt update should be applied.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionSystemPromptMode {
    /// Replace Goose's base system prompt with the provided text.
    Set,
    /// Append the provided text under Goose's "Additional Instructions" section.
    #[default]
    Append,
}

/// Set, append, or clear system prompt text for a session.
///
/// `mode: "set"` replaces goose's base system prompt. `mode: "append"` adds an
/// instruction under "Additional Instructions". Reusing a key replaces the
/// previous value for that mode/key; sending empty text clears it.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/session/system-prompt/set",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionSystemPromptRequest {
    pub session_id: String,
    #[serde(default)]
    pub mode: SessionSystemPromptMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub text: String,
}

/// Add user input to the currently active prompt without starting a new prompt.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/session/steer",
    response = SteerSessionResponse
)]
#[serde(rename_all = "camelCase")]
pub struct SteerSessionRequest {
    pub session_id: String,
    #[serde(default)]
    pub prompt: Vec<ContentBlock>,
    pub expected_run_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct SteerSessionResponse {
    pub run_id: String,
    /// Stable id of the queued steer message. The same id later appears as
    /// `messageId` on the streamed `UserMessageChunk` (with `_meta.goose.steer`),
    /// letting clients correlate a queued steer with its pickup.
    pub message_id: String,
}

/// Get a diagnostic report for a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/diagnostics/get",
    response = DiagnosticsGetResponse
)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsGetRequest {
    pub session_id: String,
    #[serde(default)]
    pub level: DiagnosticsReportLevel,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsReportLevel {
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct DiagnosticsGetResponse {
    pub report: serde_json::Value,
}

/// Information about a prompt template, including its default content and customization status.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplateEntry {
    pub name: String,
    pub description: String,
    pub default_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_content: Option<String>,
    pub is_customized: bool,
}

/// List all available goose prompt templates.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/prompts/list", response = ListPromptsResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsResponse {
    pub prompts: Vec<PromptTemplateEntry>,
}

/// Read a goose prompt template.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/prompts/get", response = GetPromptResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptRequest {
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptResponse {
    pub name: String,
    pub content: String,
    pub default_content: String,
    pub is_customized: bool,
}

/// Save a custom goose prompt template.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/prompts/save", response = PromptOperationResponse)]
#[serde(rename_all = "camelCase")]
pub struct SavePromptRequest {
    pub name: String,
    pub content: String,
}

/// Reset a goose prompt template to its default content.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/prompts/reset", response = PromptOperationResponse)]
#[serde(rename_all = "camelCase")]
pub struct ResetPromptRequest {
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct PromptOperationResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GooseExtension {
    Builtin {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bundled: Option<bool>,
        /// Tool allowlist for this extension. Omit this field to allow all tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_tools: Option<Vec<String>>,
    },
    Platform {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bundled: Option<bool>,
        /// Tool allowlist for this extension. Omit this field to allow all tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_tools: Option<Vec<String>>,
    },
    Mcp {
        server: Box<McpServer>,
        #[serde(default, rename = "envKeys", skip_serializing_if = "Vec::is_empty")]
        env_keys: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        socket: Option<String>,
        /// Pre-registered OAuth client ID for the server's authorization server.
        #[serde(default, rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        /// Name of the env/secret key holding the OAuth client secret.
        #[serde(
            default,
            rename = "clientSecretKey",
            skip_serializing_if = "Option::is_none"
        )]
        client_secret_key: Option<String>,
        /// OAuth scopes to request with `client_id`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bundled: Option<bool>,
        /// Tool allowlist for this extension. Omit this field to allow all tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_tools: Option<Vec<String>>,
    },
}

impl Default for GooseExtension {
    fn default() -> Self {
        Self::Builtin {
            name: String::new(),
            description: None,
            display_name: None,
            timeout: None,
            bundled: None,
            available_tools: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GooseExtensionEntry {
    pub extension: GooseExtension,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionExtensionEntry {
    pub extension: GooseExtension,
    pub extension_key: String,
}

/// List configured extensions and any warnings.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/config/extensions/list",
    response = GetConfigExtensionsResponse
)]
pub struct GetConfigExtensionsRequest {}

/// List configured extensions and any warnings.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct GetConfigExtensionsResponse {
    pub extensions: Vec<GooseExtensionEntry>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

pub type GetExtensionsRequest = GetConfigExtensionsRequest;
pub type GetExtensionsResponse = GetConfigExtensionsResponse;

/// Persist a new extension to the user's global goose config.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/extensions/add", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct AddConfigExtensionRequest {
    pub extension: GooseExtension,
    #[serde(default)]
    pub enabled: bool,
}

/// Remove a persisted extension from the user's global goose config.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/extensions/remove", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct RemoveConfigExtensionRequest {
    pub config_key: String,
}

/// Set the `enabled` flag for a persisted extension in the user's global goose config.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/config/extensions/set-enabled",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct SetConfigExtensionEnabledRequest {
    pub config_key: String,
    pub enabled: bool,
}

/// List extensions enabled for an active session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/extensions/list", response = GetSessionExtensionsResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionExtensionsRequest {
    pub session_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct GetSessionExtensionsResponse {
    pub extensions: Vec<SessionExtensionEntry>,
}

/// Read allowlisted user preferences. Empty `keys` means all supported preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/preferences/read", response = PreferencesReadResponse)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesReadRequest {
    #[serde(default)]
    pub keys: Vec<PreferenceKey>,
}

/// Save allowlisted user preferences.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/preferences/save", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesSaveRequest {
    #[serde(default)]
    pub values: Vec<PreferenceValue>,
}

/// Read one goose configuration value.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/read", response = ConfigReadResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReadRequest {
    pub key: String,
    #[serde(default)]
    pub is_secret: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReadResponse {
    #[serde(default)]
    pub value: serde_json::Value,
}

/// Create or replace one goose configuration value.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/upsert", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpsertRequest {
    pub key: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub is_secret: bool,
}

/// Remove one goose configuration value.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/remove", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRemoveRequest {
    pub key: String,
    #[serde(default)]
    pub is_secret: bool,
}

/// Read all non-secret goose configuration values.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/config/read-all", response = ConfigReadAllResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReadAllRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReadAllResponse {
    pub config: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PreferenceKey {
    #[default]
    AutoCompactThreshold,
    GooseThinkingEffort,
    VoiceAutoSubmitPhrases,
    VoiceDictationProvider,
    VoiceDictationPreferredMic,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceValue {
    pub key: PreferenceKey,
    #[serde(default)]
    pub value: serde_json::Value,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesReadResponse {
    pub values: Vec<PreferenceValue>,
}

/// Read goose default provider and model configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/defaults/read", response = DefaultsReadResponse)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsReadRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsReadResponse {
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

/// Save goose default provider and model configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/defaults/save", response = DefaultsReadResponse)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsSaveRequest {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Clear goose default provider and model configuration.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/defaults/clear", response = DefaultsReadResponse)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsClearRequest {}

/// Sources that onboarding knows how to discover and import.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingImportSourceKind {
    #[default]
    GooseConfig,
    ClaudeDesktop,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportCounts {
    pub providers: u32,
    pub extensions: u32,
    pub sessions: u32,
    pub skills: u32,
    pub projects: u32,
    pub preferences: u32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportCandidate {
    pub id: String,
    pub source_kind: OnboardingImportSourceKind,
    pub display_name: String,
    pub path: String,
    pub counts: OnboardingImportCounts,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Scan for existing goose and compatible app data that onboarding can import.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/onboarding/import/scan",
    response = OnboardingImportScanResponse
)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportScanRequest {
    /// Empty means all supported import sources.
    #[serde(default)]
    pub sources: Vec<OnboardingImportSourceKind>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportScanResponse {
    pub candidates: Vec<OnboardingImportCandidate>,
}

/// Import selected onboarding candidates.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/onboarding/import/apply",
    response = OnboardingImportApplyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportApplyRequest {
    #[serde(default)]
    pub candidate_ids: Vec<String>,
    #[serde(default)]
    pub enable_imported_extensions: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingImportApplyResponse {
    pub imported: OnboardingImportCounts,
    pub skipped: OnboardingImportCounts,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_defaults: Option<DefaultsReadResponse>,
}

/// Return list-style metadata for a single session without loading the conversation.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/session/info",
    response = GetSessionInfoResponse
)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionInfoRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionInfoResponse {
    pub session: SessionInfo,
}

/// Truncate a session conversation from the given message timestamp onward.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/session/conversation/truncate",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct TruncateSessionConversationRequest {
    pub session_id: String,
    pub truncate_from: i64,
}

/// Update the project association for a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/project/update", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSessionProjectRequest {
    pub session_id: String,
    pub project_id: Option<String>,
}

/// Rename a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/rename", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct RenameSessionRequest {
    pub session_id: String,
    pub title: String,
}

/// Archive a session (soft delete).
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/archive", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSessionRequest {
    pub session_id: String,
}

/// Unarchive a previously archived session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/unarchive", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct UnarchiveSessionRequest {
    pub session_id: String,
}

/// Export a session as a JSON or markdown string.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/export", response = ExportSessionResponse)]
#[serde(rename_all = "camelCase")]
pub struct ExportSessionRequest {
    pub session_id: String,
    #[serde(default)]
    pub format: SessionExportFormat,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionExportFormat {
    #[default]
    Json,
    Markdown,
}

/// Export session response — raw JSON of the goose session with `conversation`,
/// or a markdown transcript when `format` is `markdown`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct ExportSessionResponse {
    pub data: String,
}

/// Import a session from a JSON string or share link.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/session/import", response = ImportSessionResponse)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionRequest {
    pub input: String,
    pub source: SessionImportSource,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionImportSource {
    #[default]
    Auto,
    Json,
    Nostr,
}

/// Share a session through Nostr and return its share links.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/session/share/nostr",
    response = ShareSessionNostrResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionNostrRequest {
    pub session_id: String,
    pub relays: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ShareSessionNostrResponse {
    pub deeplink: String,
    pub nevent: String,
    pub event_id: String,
    pub relays: Vec<String>,
}

/// Import session response — metadata about the newly created session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionResponse {
    pub session_id: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigKey {
    pub name: String,
    pub required: bool,
    pub secret: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub oauth_flow: bool,
    #[serde(default)]
    pub device_code_flow: bool,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigFieldValueDto {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    pub is_set: bool,
    pub is_secret: bool,
    pub required: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigStatusDto {
    pub provider_id: String,
    pub is_configured: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigFieldUpdate {
    pub key: String,
    pub value: String,
}

/// Read saved configuration field values for one provider.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/config/read",
    response = ProviderConfigReadResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigReadRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigReadResponse {
    pub fields: Vec<ProviderConfigFieldValueDto>,
}

/// Return provider configured statuses. Empty provider_ids means all providers.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/config/status",
    response = ProviderConfigStatusResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigStatusRequest {
    #[serde(default)]
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigStatusResponse {
    pub statuses: Vec<ProviderConfigStatusDto>,
}

/// Save provider configuration fields and start an inventory refresh when supported.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/config/save",
    response = ProviderConfigChangeResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigSaveRequest {
    pub provider_id: String,
    pub fields: Vec<ProviderConfigFieldUpdate>,
}

/// Delete provider configuration fields and start an inventory refresh when supported.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/config/delete",
    response = ProviderConfigChangeResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigDeleteRequest {
    pub provider_id: String,
}

/// Run a provider-owned native authentication flow and start an inventory refresh when supported.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/config/authenticate",
    response = ProviderConfigChangeResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigAuthenticateRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigChangeResponse {
    pub status: ProviderConfigStatusDto,
    pub refresh: RefreshProviderInventoryResponse,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSecretStorageDto {
    #[default]
    SecretStore,
    ProviderCache,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSecretStatusDto {
    Valid,
    Expired,
    #[default]
    Unknown,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretDto {
    pub id: String,
    pub provider: String,
    pub provider_display_name: String,
    pub name: String,
    pub storage: ProviderSecretStorageDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub status: ProviderSecretStatusDto,
    pub configured: bool,
    pub has_secret: bool,
    pub can_delete: bool,
    pub can_configure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configure_provider: Option<String>,
}

/// List provider credentials stored locally by goose.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/secrets/list",
    response = ProviderSecretsListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretsListRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretsListResponse {
    pub secrets: Vec<ProviderSecretDto>,
}

/// Delete a locally stored provider credential by id.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/secrets/delete",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretDeleteRequest {
    pub id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalModelInfoDto {
    pub provider: String,
    pub model: String,
    pub context_limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_token_cost: Option<f64>,
    pub currency: String,
}

/// Look up canonical (bundled-registry) model info for a provider/model pair.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/canonical-model-info",
    response = CanonicalModelInfoResponse
)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalModelInfoRequest {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalModelInfoResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_info: Option<CanonicalModelInfoDto>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplateCatalogEntryDto {
    pub provider_id: String,
    pub name: String,
    pub format: String,
    pub api_url: String,
    pub model_count: usize,
    pub doc_url: String,
    pub env_var: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetupCategoryDto {
    Agent,
    #[default]
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetupMethodDto {
    None,
    SingleApiKey,
    ConfigFields,
    HostWithOauthFallback,
    OauthBrowser,
    OauthDeviceCode,
    CloudCredentials,
    Local,
    CliAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetupGroupDto {
    Default,
    Additional,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupFieldDto {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupCatalogEntryDto {
    pub provider_id: String,
    pub name: String,
    pub category: ProviderSetupCategoryDto,
    /// Whether this provider communicates through ACP.
    #[serde(default)]
    pub acp: bool,
    pub description: String,
    pub setup_method: ProviderSetupMethodDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_connect_query: Option<String>,
    #[serde(default)]
    pub fields: Vec<ProviderSetupFieldDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_url: Option<String>,
    pub group: ProviderSetupGroupDto,
    pub show_only_when_installed: bool,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub supports_install: bool,
    pub supports_auth: bool,
    pub supports_auth_status: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplateCapabilitiesDto {
    pub tool_call: bool,
    pub reasoning: bool,
    pub attachment: bool,
    pub temperature: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplateModelDto {
    pub id: String,
    pub name: String,
    pub context_limit: usize,
    pub capabilities: ProviderTemplateCapabilitiesDto,
    pub deprecated: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplateDto {
    pub provider_id: String,
    pub name: String,
    pub format: String,
    pub api_url: String,
    pub models: Vec<ProviderTemplateModelDto>,
    pub supports_streaming: bool,
    pub env_var: String,
    pub doc_url: String,
}

/// List custom-provider catalog entries. Omit `format` to list all formats.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/catalog/list",
    response = ProviderCatalogListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogListResponse {
    pub providers: Vec<ProviderTemplateCatalogEntryDto>,
}

/// List provider setup catalog entries
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/setup/catalog/list",
    response = ProviderSetupCatalogListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupCatalogListRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetupCatalogListResponse {
    pub providers: Vec<ProviderSetupCatalogEntryDto>,
}

/// Return the editable template for one catalog provider.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/catalog/template",
    response = ProviderCatalogTemplateResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogTemplateRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogTemplateResponse {
    pub template: ProviderTemplateDto,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderConfigDto {
    pub provider_id: String,
    pub engine: String,
    pub display_name: String,
    pub api_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_streaming: Option<bool>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub requires_auth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
    pub toolshim: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    pub api_key_set: bool,
    pub preserves_thinking: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderUpsertDto {
    pub engine: String,
    pub display_name: String,
    pub api_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_streaming: Option<bool>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub requires_auth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserves_thinking: Option<bool>,
}

/// Create a custom provider backed by goose's declarative provider store.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/custom/create",
    response = CustomProviderCreateResponse
)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderCreateRequest {
    #[serde(flatten)]
    pub provider: CustomProviderUpsertDto,
    pub toolshim: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderCreateResponse {
    pub provider_id: String,
    pub status: ProviderConfigStatusDto,
    pub refresh: RefreshProviderInventoryResponse,
}

/// Read a declarative provider config. Custom configs are editable; bundled configs are read-only.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/custom/read",
    response = CustomProviderReadResponse
)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderReadRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderReadResponse {
    pub provider: CustomProviderConfigDto,
    pub editable: bool,
    pub status: ProviderConfigStatusDto,
}

/// Update a custom provider backed by goose's declarative provider store.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/custom/update",
    response = CustomProviderUpdateResponse
)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderUpdateRequest {
    pub provider_id: String,
    #[serde(flatten)]
    pub provider: CustomProviderUpsertDto,
    pub toolshim: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderUpdateResponse {
    pub provider_id: String,
    pub status: ProviderConfigStatusDto,
    pub refresh: RefreshProviderInventoryResponse,
}

/// Delete a custom provider from goose's declarative provider store.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/custom/delete",
    response = CustomProviderDeleteResponse
)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderDeleteRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderDeleteResponse {
    pub provider_id: String,
    pub refresh: RefreshProviderInventoryResponse,
}

/// The type of source entity.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SourceType {
    #[default]
    Skill,
    BuiltinSkill,
    Recipe,
    Subrecipe,
    Agent,
    Project,
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceType::Skill => write!(f, "skill"),
            SourceType::BuiltinSkill => write!(f, "builtin skill"),
            SourceType::Recipe => write!(f, "recipe"),
            SourceType::Subrecipe => write!(f, "subrecipe"),
            SourceType::Agent => write!(f, "agent"),
            SourceType::Project => write!(f, "project"),
        }
    }
}

/// A source discovered by Goose. Filesystem sources use an on-disk path;
/// built-in sources use a stable synthetic path. Sources may be either
/// `global` (shared across all projects) or project-specific.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceEntry {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub name: String,
    pub description: String,
    pub content: String,
    /// Stable on-disk path identifying this source. Pass it back to
    /// update/delete/export to operate on this entry. Skills use the directory
    /// containing `SKILL.md`; projects use the project file path; built-in
    /// skills use `builtin://skills/<name>` synthetic paths.
    pub path: String,
    /// True when the source lives in the user's global sources directory; false
    /// when it lives inside a specific project.
    pub global: bool,
    /// True when this source can be modified through source CRUD methods.
    /// Client-provided bundled sources are returned as read-only.
    #[serde(default)]
    pub writable: bool,
    /// Paths (absolute) of additional files that live alongside the source.
    /// Only skills currently populate this; empty for other source types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supporting_files: Vec<String>,
    /// Arbitrary key/value pairs for type-specific metadata (e.g. icon, color,
    /// preferredProvider for projects). Stored in the frontmatter.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}

impl SourceEntry {
    /// Render this source as a markdown block suitable for injecting into an
    /// LLM context. Used by the skills and summon runtimes when loading a
    /// source into the current conversation.
    pub fn to_load_text(&self) -> String {
        format!(
            "## {} ({})\n\n{}\n\n### Content\n\n{}",
            self.name, self.source_type, self.description, self.content
        )
    }
}

/// Target scope for creating or importing sources.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "camelCase")]
pub enum SourceScope {
    #[default]
    Global,
    ProjectDir {
        #[serde(rename = "projectDir")]
        project_dir: String,
    },
    ProjectId {
        #[serde(rename = "projectId")]
        project_id: String,
    },
}

/// Create a new source in an explicit target scope (global or project-scoped).
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/create", response = CreateSourceResponse)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceRequest {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub name: String,
    pub description: String,
    pub content: String,
    pub target: SourceScope,
    /// Arbitrary key/value metadata.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceResponse {
    pub source: SourceEntry,
}

/// List discovered sources.
///
/// If `type` is omitted or `skill`, this lists filesystem/plugin skills only.
/// Both global and project-scoped skills are included when `project_dir` is
/// set. If `type` is `builtinSkill`, this lists shipped read-only built-in
/// skills.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/list", response = ListSourcesResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListSourcesRequest {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<SourceType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    /// When true, also scan the working directories of all known projects for
    /// project-scoped sources (e.g. skills stored under `{workingDir}/.agents/skills/`).
    #[serde(default)]
    pub include_project_sources: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListSourcesResponse {
    pub sources: Vec<SourceEntry>,
}

/// A user-facing `@` mention target backed by an agent, recipe, or subrecipe source.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentMention {
    pub name: String,
    pub description: String,
    pub source_type: SourceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub mention: String,
}

/// List user-facing agent mention targets for `@` autocomplete.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/agent-mentions/list",
    response = ListAgentMentionsResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentMentionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentMentionsResponse {
    pub agents: Vec<AgentMention>,
}

/// List slash commands available for `/` autocomplete.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/slash-commands/list",
    response = ListSlashCommandsResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ListSlashCommandsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListSlashCommandsResponse {
    pub available_commands: Vec<AvailableCommand>,
}

/// Update an existing source's name, description, and content by absolute path.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/update", response = UpdateSourceResponse)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceRequest {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub path: String,
    pub name: String,
    pub description: String,
    pub content: String,
    /// When `Some`, replaces all stored properties on the source. When
    /// `None` (or omitted), the source's existing properties are
    /// preserved. Callers that don't model the full property bag (e.g.
    /// the skills editor, which only edits name/description/content)
    /// should omit this so per-skill metadata isn't silently erased.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceResponse {
    pub source: SourceEntry,
}

/// Delete a source and its on-disk directory by absolute path.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/delete", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSourceRequest {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub path: String,
}

/// Export a source at an absolute path as a portable JSON payload.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/export", response = ExportSourceResponse)]
#[serde(rename_all = "camelCase")]
pub struct ExportSourceRequest {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub path: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ExportSourceResponse {
    pub json: String,
    pub filename: String,
}

/// Import a source from a JSON export payload produced by `_goose/unstable/sources/export`.
/// The imported source is written into the explicit target scope; on name
/// collisions a `-imported` suffix is appended.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/sources/import", response = ImportSourcesResponse)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourcesRequest {
    pub data: String,
    pub target: SourceScope,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ImportSourcesResponse {
    pub sources: Vec<SourceEntry>,
}

/// Transcribe audio via a dictation provider.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/dictation/transcribe", response = DictationTranscribeResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationTranscribeRequest {
    /// Base64-encoded audio data
    pub audio: String,
    /// MIME type (e.g. "audio/wav", "audio/webm")
    pub mime_type: String,
    /// Provider to use: "openai", "groq", "elevenlabs", or "local"
    pub provider: String,
}

/// Transcription result.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct DictationTranscribeResponse {
    pub text: String,
}

/// Get the configuration status of all dictation providers.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/dictation/config", response = DictationConfigResponse)]
pub struct DictationConfigRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DictationModelOption {
    pub id: String,
    pub label: String,
    pub description: String,
}

/// Per-provider configuration status.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DictationProviderStatusEntry {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub description: String,
    pub uses_provider_config: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_config_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub available_models: Vec<DictationModelOption>,
}

/// Dictation config response — map of provider name to status.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct DictationConfigResponse {
    pub providers: HashMap<String, DictationProviderStatusEntry>,
}

/// List providers with setup metadata and the current model inventory snapshot.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/providers/list", response = ListProvidersResponse)]
#[serde(rename_all = "camelCase")]
pub struct ListProvidersRequest {
    /// Only return entries for these providers. Empty means all.
    #[serde(default)]
    pub provider_ids: Vec<String>,
}

/// Provider list response.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct ListProvidersResponse {
    pub entries: Vec<ProviderInventoryEntryDto>,
}

/// Check whether an ACP provider can initialize and create a session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/readiness/check",
    response = ProviderReadinessCheckResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReadinessCheckRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReadinessCheckResponse {
    pub provider_id: String,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// List the raw model identifiers returned by a provider's live supported-models API.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/supported-models/list",
    response = ProviderSupportedModelsListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSupportedModelsListRequest {
    pub provider_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSupportedModelsListResponse {
    pub provider_id: String,
    pub models: Vec<String>,
}

/// Trigger a background refresh of provider inventories.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/providers/inventory/refresh",
    response = RefreshProviderInventoryResponse
)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProviderInventoryRequest {
    /// Which providers to refresh. Empty means all known providers.
    #[serde(default)]
    pub provider_ids: Vec<String>,
}

/// Refresh acknowledgement.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProviderInventoryResponse {
    /// Which providers will be refreshed.
    pub started: Vec<String>,
    /// Which providers were skipped and why.
    #[serde(default)]
    pub skipped: Vec<RefreshProviderInventorySkipDto>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefreshProviderInventorySkipDto {
    pub provider_id: String,
    pub reason: RefreshProviderInventorySkipReasonDto,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefreshProviderInventorySkipReasonDto {
    #[default]
    UnknownProvider,
    NotConfigured,
    DoesNotSupportRefresh,
    AlreadyRefreshing,
}

/// A single model in provider inventory.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInventoryModelDto {
    /// Model identifier as the provider knows it.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Model family for grouping in UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Context window size in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<usize>,
    /// Whether the model supports reasoning/extended thinking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Whether this model should appear in the compact recommended picker.
    #[serde(default)]
    pub recommended: bool,
}

/// Provider inventory entry.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInventoryEntryDto {
    /// Provider identifier.
    pub provider_id: String,
    /// Human-readable provider name.
    pub provider_name: String,
    /// Description of the provider's capabilities.
    pub description: String,
    /// The default/recommended model for this provider.
    pub default_model: String,
    /// Whether Goose has enough configuration to use this provider.
    pub configured: bool,
    /// Whether the provider's external runtime or required configuration is available.
    pub available: bool,
    /// Provider classification such as `Preferred`, `Builtin`, `Declarative`, or `Custom`.
    pub provider_type: String,
    /// Whether this provider communicates through ACP.
    #[serde(default)]
    pub acp: bool,
    /// Whether this provider should appear in normal provider setup UIs.
    pub visible_in_setup: bool,
    /// Whether this provider is retained only for compatibility.
    pub deprecated: bool,
    /// Preferred replacement for a deprecated provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// Required configuration keys and setup metadata.
    pub config_keys: Vec<ProviderConfigKey>,
    /// Step-by-step setup instructions, when present.
    pub setup_steps: Vec<String>,
    /// Whether this provider supports background inventory refresh.
    pub supports_refresh: bool,
    /// Whether a refresh is currently in flight.
    pub refreshing: bool,
    /// The list of available models.
    pub models: Vec<ProviderInventoryModelDto>,
    /// When this entry was last successfully refreshed (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
    /// When a refresh was most recently attempted (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh_attempt_at: Option<String>,
    /// The last refresh failure message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh_error: Option<String>,
    /// Whether we believe this data may be outdated.
    pub stale: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocalInferenceToolCallingMode {
    #[default]
    Auto,
    ForceNative,
    ForceEmulated,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalInferenceChatTemplate {
    #[default]
    Embedded,
    Builtin {
        name: String,
    },
    CustomInline {
        template: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum LocalInferenceSamplingConfig {
    Greedy,
    Temperature {
        temperature: f32,
        top_k: i32,
        top_p: f32,
        min_p: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u32>,
    },
    MirostatV2 {
        tau: f32,
        eta: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u32>,
    },
}

impl Default for LocalInferenceSamplingConfig {
    fn default() -> Self {
        Self::Temperature {
            temperature: 0.8,
            top_k: 40,
            top_p: 0.95,
            min_p: 0.05,
            seed: None,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelSettingsDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_model: Option<String>,
    #[serde(default)]
    pub sampling: LocalInferenceSamplingConfig,
    pub repeat_penalty: f32,
    pub repeat_last_n: i32,
    pub frequency_penalty: f32,
    pub presence_penalty: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_batch: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_gpu_layers: Option<u32>,
    pub use_mlock: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_attention: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_threads: Option<i32>,
    #[serde(default)]
    pub tool_calling: LocalInferenceToolCallingMode,
    #[serde(default)]
    pub chat_template: LocalInferenceChatTemplate,
    pub enable_thinking: bool,
    pub vision_capable: bool,
    pub image_token_estimate: usize,
    pub mmproj_size_bytes: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum LocalInferenceDownloadState {
    #[default]
    NotDownloaded,
    Downloading,
    Downloaded,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadStatusDto {
    pub state: LocalInferenceDownloadState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_downloaded: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceDownloadProgressDto {
    pub model_id: String,
    pub status: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub progress_percent: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub task_exited: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDto {
    pub id: String,
    pub repo_id: String,
    pub filename: String,
    pub quantization: String,
    pub size_bytes: u64,
    pub status: LocalInferenceModelDownloadStatusDto,
    pub recommended: bool,
    pub is_loaded: bool,
    pub settings: LocalInferenceModelSettingsDto,
    pub vision_capable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj_status: Option<LocalInferenceModelDownloadStatusDto>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHfModelVariantDto {
    pub variant_id: String,
    pub label: String,
    pub backend_id: String,
    pub format: String,
    pub model_id: String,
    pub download_id: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    pub description: String,
    pub quality_rank: u8,
    pub sharded: bool,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHfGgufFileDto {
    pub filename: String,
    pub size_bytes: u64,
    pub quantization: String,
    pub download_url: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHfModelInfoDto {
    pub repo_id: String,
    pub author: String,
    pub model_name: String,
    pub downloads: u64,
    #[serde(default)]
    pub gguf_files: Vec<LocalInferenceHfGgufFileDto>,
    #[serde(default)]
    pub variants: Vec<LocalInferenceHfModelVariantDto>,
}

/// List locally available inference models.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/list",
    response = LocalInferenceModelsListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelsListRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelsListResponse {
    pub models: Vec<LocalInferenceModelDto>,
}

/// Download a model for local inference.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/download",
    response = LocalInferenceModelDownloadResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadRequest {
    pub spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadResponse {
    pub model_id: String,
}

/// Get the progress of a local model download.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/download/progress",
    response = LocalInferenceModelDownloadProgressResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadProgressRequest {
    pub model_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadProgressResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<LocalInferenceDownloadProgressDto>,
}

/// Cancel a local model download.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/download/cancel",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDownloadCancelRequest {
    pub model_id: String,
}

/// Delete a downloaded local inference model.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/delete",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelDeleteRequest {
    pub model_id: String,
}

/// Evict a local inference model from memory.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/evict",
    response = EmptyResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelEvictRequest {
    pub model_id: String,
}

/// Read the sampling settings for a local inference model.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/settings/read",
    response = LocalInferenceModelSettingsReadResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelSettingsReadRequest {
    pub model_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelSettingsReadResponse {
    pub settings: LocalInferenceModelSettingsDto,
}

/// Update the sampling settings for a local inference model.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/models/settings/update",
    response = LocalInferenceModelSettingsUpdateResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelSettingsUpdateRequest {
    pub model_id: String,
    pub settings: LocalInferenceModelSettingsDto,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceModelSettingsUpdateResponse {
    pub settings: LocalInferenceModelSettingsDto,
}

/// Search Hugging Face for local inference models.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/huggingface/search",
    response = LocalInferenceHuggingFaceSearchResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHuggingFaceSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHuggingFaceSearchResponse {
    pub models: Vec<LocalInferenceHfModelInfoDto>,
}

/// List downloadable variants of a Hugging Face model repository.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/huggingface/repo/variants",
    response = LocalInferenceHuggingFaceRepoVariantsResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHuggingFaceRepoVariantsRequest {
    pub repo_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceHuggingFaceRepoVariantsResponse {
    pub variants: Vec<LocalInferenceHfModelVariantDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_index: Option<usize>,
    pub available_memory_bytes: u64,
    pub downloaded_quants: Vec<String>,
    pub downloaded_variants: Vec<String>,
}

/// List built-in chat templates for local inference.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/local-inference/chat-templates/builtin/list",
    response = LocalInferenceBuiltinChatTemplatesListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceBuiltinChatTemplatesListRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct LocalInferenceBuiltinChatTemplatesListResponse {
    pub templates: Vec<String>,
}

/// Empty success response for operations that return no data.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct EmptyResponse {}

/// List available local Whisper models with their download status.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/dictation/models/list",
    response = DictationModelsListResponse
)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelsListRequest {}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelsListResponse {
    pub models: Vec<DictationLocalModelStatus>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DictationLocalModelStatus {
    pub id: String,
    pub label: String,
    pub description: String,
    pub size_mb: u32,
    pub downloaded: bool,
    pub download_in_progress: bool,
    pub recommended: bool,
}

/// Kick off a background download of a local Whisper model.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/dictation/models/download", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelDownloadRequest {
    pub model_id: String,
}

/// Poll the progress of an in-flight download.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(
    method = "_goose/unstable/dictation/models/download/progress",
    response = DictationModelDownloadProgressResponse
)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelDownloadProgressRequest {
    pub model_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelDownloadProgressResponse {
    /// None when no download is active for this model id.
    pub progress: Option<DictationDownloadProgress>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DictationDownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub progress_percent: f32,
    /// serde lowercase of DownloadStatus: "downloading" | "completed" | "failed" | "cancelled"
    pub status: String,
    pub error: Option<String>,
}

/// Cancel an in-flight download.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/dictation/models/cancel", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelCancelRequest {
    pub model_id: String,
}

/// Delete a downloaded local Whisper model from disk.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/dictation/models/delete", response = EmptyResponse)]
#[serde(rename_all = "camelCase")]
pub struct DictationModelDeleteRequest {
    pub model_id: String,
}

/// Permission level for a tool.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionLevel {
    AlwaysAllow,
    #[default]
    AskBefore,
    NeverAllow,
}

/// A single tool permission entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPermissionEntry {
    pub tool_name: String,
    pub permission: ToolPermissionLevel,
}

/// Set permission levels for one or more tools.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_goose/unstable/tools/permissions/set", response = SetToolPermissionsResponse)]
#[serde(rename_all = "camelCase")]
pub struct SetToolPermissionsRequest {
    pub tool_permissions: Vec<ToolPermissionEntry>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
pub struct SetToolPermissionsResponse {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_session_request_defaults_to_json_without_format() {
        let req: ExportSessionRequest = serde_json::from_str(r#"{"sessionId":"abc"}"#).unwrap();

        assert_eq!(req.format, SessionExportFormat::Json);
    }

    #[test]
    fn export_session_request_accepts_markdown_format() {
        let req: ExportSessionRequest =
            serde_json::from_str(r#"{"sessionId":"abc","format":"markdown"}"#).unwrap();

        assert_eq!(req.format, SessionExportFormat::Markdown);
    }
}
