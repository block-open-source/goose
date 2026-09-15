use crate::conversation::token_usage::{CostSource, ProviderUsage};
use crate::conversation::tool_result_serde;
use crate::mcp_utils::extract_text_from_resource;
use crate::utils::sanitize_unicode_tags;
use base64::Engine;
use chrono::Utc;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ElicitationAction, ImageContent,
    JsonObject, PromptMessage, ResourceContents, Role, TextContent,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::fmt;
use uuid::Uuid;

pub enum ToolCallResult<T> {
    Success { value: T },
    Error { error: String },
}

/// Custom deserializer for MessageContent that sanitizes Unicode Tags in text content
fn deserialize_sanitized_content<'de, D>(
    deserializer: D,
) -> Result<Vec<MessageContentBlock>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;

    let mut migrated = Vec::with_capacity(raw.len());
    for item in raw {
        match item.get("type").and_then(|v| v.as_str()) {
            // Filter out old "conversationCompacted" messages from pre-14.0
            Some("conversationCompacted") => {}
            // Migrate old "reasoning" content to "thinking". Invalid legacy reasoning
            // blocks are dropped so they don't fail deserialization.
            Some("reasoning") => {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    migrated.push(serde_json::json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": ""
                    }));
                }
            }
            _ => migrated.push(item),
        }
    }

    let mut content: Vec<MessageContentBlock> =
        serde_json::from_value(serde_json::Value::Array(migrated))
            .map_err(|e| Error::custom(format!("Failed to deserialize MessageContent: {}", e)))?;

    for message_content in &mut content {
        match message_content {
            MessageContentBlock::Text(text_content) => {
                let original = &text_content.text;
                let sanitized = sanitize_unicode_tags(original);
                if *original != sanitized {
                    tracing::info!(
                        original = %original,
                        sanitized = %sanitized,
                        removed_count = original.len() - sanitized.len(),
                        "Unicode Tags sanitized during Message deserialization"
                    );
                    text_content.text = sanitized;
                }
            }
            MessageContentBlock::ToolResponse(response) => {
                sanitize_tool_result_in_place(&mut response.tool_result);
            }
            _ => {}
        }
    }

    Ok(content)
}

/// Provider-specific metadata for tool requests/responses.
/// Allows providers to store custom data without polluting the core model.
pub type ProviderMetadata = serde_json::Map<String, serde_json::Value>;
pub type ToolResult<T> = Result<T, rmcp::model::ErrorData>;

pub(crate) fn sanitize_tool_result_in_place(tool_result: &mut ToolResult<CallToolResult>) {
    match tool_result {
        Ok(result) => {
            for content in &mut result.content {
                match content {
                    ContentBlock::Text(text) => {
                        text.text = sanitize_unicode_tags(&text.text);
                    }
                    ContentBlock::Resource(resource) => match &mut resource.resource {
                        ResourceContents::TextResourceContents { text, .. } => {
                            *text = sanitize_unicode_tags(text);
                        }
                        ResourceContents::BlobResourceContents { blob, .. } => {
                            let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(blob.as_bytes())
                            else {
                                *blob = sanitize_unicode_tags(blob);
                                continue;
                            };
                            let Ok(text) = String::from_utf8(bytes) else {
                                continue;
                            };
                            let sanitized = sanitize_unicode_tags(&text);
                            if text != sanitized {
                                *blob = base64::engine::general_purpose::STANDARD
                                    .encode(sanitized.as_bytes());
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        Err(error) => {
            error.message = sanitize_unicode_tags(error.message.as_ref()).into();
        }
    }
}

fn sanitize_tool_result(mut tool_result: ToolResult<CallToolResult>) -> ToolResult<CallToolResult> {
    sanitize_tool_result_in_place(&mut tool_result);
    tool_result
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRequest {
    pub id: String,
    #[serde(with = "tool_result_serde")]
    pub tool_call: ToolResult<CallToolRequestParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProviderMetadata>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub tool_meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolNameParts<'a> {
    pub extension_name: Option<&'a str>,
    pub tool_name: &'a str,
}

/// A chain summary persisted on the first tool request of a chain.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolChainSummary {
    pub summary: String,
    pub count: usize,
}

/// Marker key under `ToolRequest.tool_meta` indicating the tool was already
/// executed externally; the agent loop must skip redispatch.
pub const TOOL_META_EXTERNAL_DISPATCH_KEY: &str = "goose.external_dispatch";

/// Key under `ToolRequest.tool_meta` storing the LLM-generated short title
/// for this tool call. Used to make the title survive session reload.
pub const TOOL_META_TITLE_KEY: &str = "goose.toolSummary.title";

/// Key under `ToolRequest.tool_meta` storing the provider-reported index of the
/// tool call within the streamed response. Streaming clients need this to
/// correlate incremental argument fragments with the right call when a model
/// emits several tool calls in parallel.
pub const TOOL_META_PROVIDER_INDEX_KEY: &str = "goose.toolCall.providerIndex";

/// Key under `ToolRequest.tool_meta` storing the LLM-generated chain summary
/// for the chain that starts at this tool request. Shape: `{ "summary": String,
/// "count": u64 }`. Only attached to the FIRST tool request in a chain.
pub const TOOL_META_CHAIN_SUMMARY_KEY: &str = "goose.toolChain.summary";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponse {
    pub id: String,
    #[serde(with = "tool_result_serde::call_tool_result")]
    pub tool_result: ToolResult<CallToolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProviderMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfirmationRequest {
    pub id: String,
    pub tool_name: String,
    pub arguments: JsonObject,
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "actionType", rename_all = "camelCase")]
pub enum ActionRequiredData {
    #[serde(rename_all = "camelCase")]
    ToolConfirmation {
        id: String,
        tool_name: String,
        arguments: JsonObject,
        prompt: Option<String>,
    },
    Elicitation {
        id: String,
        message: String,
        requested_schema: serde_json::Value,
    },
    ElicitationResponse {
        id: String,
        user_data: serde_json::Value,
        #[serde(default = "default_elicitation_action")]
        action: ElicitationAction,
    },
    ToolConfirmationResponse {
        id: String,
        permission: crate::permission::Permission,
    },
}

fn default_elicitation_action() -> ElicitationAction {
    ElicitationAction::Accept
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequired {
    pub data: ActionRequiredData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingContentBlock {
    pub thinking: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactedThinkingContentBlock {
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemNotificationType {
    ThinkingMessage,
    ProgressMessage,
    InlineMessage,
    CreditsExhausted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemNotificationContent {
    pub notification_type: SystemNotificationType,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageErrorKind {
    Authentication,
    ContextLengthExceeded,
    CreditsExhausted,
    #[serde(other)]
    Other,
}

impl From<&crate::errors::ProviderError> for MessageErrorKind {
    fn from(err: &crate::errors::ProviderError) -> Self {
        use crate::errors::ProviderError;
        match err {
            ProviderError::Authentication(_) => MessageErrorKind::Authentication,
            ProviderError::ContextLengthExceeded(_) => MessageErrorKind::ContextLengthExceeded,
            ProviderError::CreditsExhausted { .. } => MessageErrorKind::CreditsExhausted,
            _ => MessageErrorKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorContent {
    pub kind: MessageErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentContent {
    pub data: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl DocumentContent {
    pub fn new<S: Into<String>, T: Into<String>>(data: S, mime_type: T) -> Self {
        Self {
            data: data.into(),
            mime_type: mime_type.into(),
            name: None,
        }
    }

    pub fn with_name<S: Into<String>>(mut self, name: S) -> Self {
        self.name = Some(name.into());
        self
    }
}

pub type MessageContent = MessageContentBlock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Content passed inside a message, which can be both simple content and tool content
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessageContentBlock {
    Text(TextContent),
    Image(ImageContent),
    Document(DocumentContent),
    ToolRequest(ToolRequest),
    ToolResponse(ToolResponse),
    ToolConfirmationRequest(ToolConfirmationRequest),
    ActionRequired(ActionRequired),
    Thinking(ThinkingContentBlock),
    RedactedThinking(RedactedThinkingContentBlock),
    SystemNotification(SystemNotificationContent),
    Error(ErrorContent),
}

impl fmt::Display for MessageContentBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageContentBlock::Text(t) => write!(f, "{}", t.text),
            MessageContentBlock::Image(i) => write!(f, "[Image: {}]", i.mime_type),
            MessageContentBlock::Document(d) => match &d.name {
                Some(name) => write!(f, "[Document: {} ({})]", name, d.mime_type),
                None => write!(f, "[Document: {}]", d.mime_type),
            },
            MessageContentBlock::ToolRequest(r) => {
                write!(f, "[ToolRequest: {}]", r.to_readable_string())
            }
            MessageContentBlock::ToolResponse(r) => write!(
                f,
                "[ToolResponse: {}]",
                match &r.tool_result {
                    Ok(result) => format!("{} content item(s)", result.content.len()),
                    Err(e) => format!("Error: {e}"),
                }
            ),
            MessageContentBlock::ToolConfirmationRequest(r) => {
                write!(f, "[ToolConfirmationRequest: {}]", r.tool_name)
            }
            MessageContentBlock::ActionRequired(a) => match &a.data {
                ActionRequiredData::ToolConfirmation { tool_name, .. } => {
                    write!(f, "[ActionRequired: ToolConfirmation for {}]", tool_name)
                }
                ActionRequiredData::Elicitation { message, .. } => {
                    write!(f, "[ActionRequired: Elicitation - {}]", message)
                }
                ActionRequiredData::ElicitationResponse { id, .. } => {
                    write!(f, "[ActionRequired: ElicitationResponse for {}]", id)
                }
                ActionRequiredData::ToolConfirmationResponse { id, .. } => {
                    write!(f, "[ActionRequired: ToolConfirmationResponse for {}]", id)
                }
            },
            MessageContentBlock::Thinking(t) => write!(f, "[Thinking: {}]", t.thinking),
            MessageContentBlock::RedactedThinking(_r) => write!(f, "[RedactedThinking]"),
            MessageContentBlock::SystemNotification(r) => {
                write!(f, "[SystemNotification: {}]", r.msg)
            }
            MessageContentBlock::Error(e) => write!(f, "[Error: {}]", e.message),
        }
    }
}

fn content_audience(content: &ContentBlock) -> Option<&Vec<Role>> {
    match content {
        ContentBlock::Text(text) => text.annotations.as_ref()?.audience.as_ref(),
        ContentBlock::Image(image) => image.annotations.as_ref()?.audience.as_ref(),
        ContentBlock::Audio(audio) => audio.annotations.as_ref()?.audience.as_ref(),
        ContentBlock::Resource(resource) => resource.annotations.as_ref()?.audience.as_ref(),
        ContentBlock::ResourceLink(resource) => resource.annotations.as_ref()?.audience.as_ref(),
        _ => None,
    }
}

impl MessageContentBlock {
    pub fn text<S: Into<String>>(text: S) -> Self {
        MessageContentBlock::Text(TextContent::new(text))
    }

    pub fn filter_for_audience(&self, audience: Role) -> Option<MessageContentBlock> {
        match self {
            MessageContentBlock::Text(text) => {
                if text
                    .annotations
                    .as_ref()
                    .and_then(|a| a.audience.as_ref())
                    .map(|roles| roles.contains(&audience))
                    .unwrap_or(true)
                {
                    Some(self.clone())
                } else {
                    None
                }
            }
            MessageContentBlock::Image(img) => {
                if img
                    .annotations
                    .as_ref()
                    .and_then(|a| a.audience.as_ref())
                    .map(|roles| roles.contains(&audience))
                    .unwrap_or(true)
                {
                    Some(self.clone())
                } else {
                    None
                }
            }
            MessageContentBlock::ToolResponse(res) => {
                let Ok(result) = &res.tool_result else {
                    return Some(self.clone());
                };

                let filtered_content: Vec<ContentBlock> = result
                    .content
                    .iter()
                    .filter(|c| {
                        content_audience(c)
                            .map(|roles| roles.contains(&audience))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();

                // Preserve ToolResponse even when content is empty - some providers
                // (like Google) need to handle empty tool responses specially
                let mut tool_result = result.clone();
                tool_result.content = filtered_content;
                Some(MessageContentBlock::ToolResponse(ToolResponse {
                    id: res.id.clone(),
                    tool_result: Ok(tool_result),
                    metadata: res.metadata.clone(),
                }))
            }
            MessageContentBlock::Thinking(_) | MessageContentBlock::RedactedThinking(_) => {
                if audience == Role::Assistant {
                    Some(self.clone())
                } else {
                    None
                }
            }
            _ => Some(self.clone()),
        }
    }

    pub fn user_visible_content(&self) -> Option<MessageContent> {
        match self {
            MessageContentBlock::Text(_)
            | MessageContentBlock::Image(_)
            | MessageContentBlock::ToolResponse(_) => self.filter_for_audience(Role::User),
            _ => Some(self.clone()),
        }
    }

    pub fn image<S: Into<String>, T: Into<String>>(data: S, mime_type: T) -> Self {
        MessageContentBlock::Image(ImageContent::new(data, mime_type))
    }

    pub fn document<S: Into<String>, T: Into<String>>(
        data: S,
        mime_type: T,
        name: Option<String>,
    ) -> Self {
        let document = DocumentContent::new(data, mime_type);
        MessageContentBlock::Document(match name {
            Some(name) => document.with_name(name),
            None => document,
        })
    }

    pub fn tool_request<S: Into<String>>(
        id: S,
        tool_call: ToolResult<CallToolRequestParams>,
    ) -> Self {
        MessageContentBlock::ToolRequest(ToolRequest {
            id: id.into(),
            tool_call,
            metadata: None,
            tool_meta: None,
        })
    }

    pub fn tool_request_with_metadata<S: Into<String>>(
        id: S,
        tool_call: ToolResult<CallToolRequestParams>,
        metadata: Option<&ProviderMetadata>,
    ) -> Self {
        MessageContentBlock::ToolRequest(ToolRequest {
            id: id.into(),
            tool_call,
            metadata: metadata.cloned(),
            tool_meta: None,
        })
    }

    pub fn tool_request_with_provider_index<S: Into<String>>(
        id: S,
        tool_call: ToolResult<CallToolRequestParams>,
        metadata: Option<&ProviderMetadata>,
        provider_index: i32,
    ) -> Self {
        MessageContentBlock::ToolRequest(ToolRequest {
            id: id.into(),
            tool_call,
            metadata: metadata.cloned(),
            tool_meta: Some(serde_json::json!({
                TOOL_META_PROVIDER_INDEX_KEY: provider_index,
            })),
        })
    }

    pub fn tool_response<S: Into<String>>(id: S, tool_result: ToolResult<CallToolResult>) -> Self {
        MessageContentBlock::ToolResponse(ToolResponse {
            id: id.into(),
            tool_result: sanitize_tool_result(tool_result),
            metadata: None,
        })
    }

    pub fn tool_response_with_metadata<S: Into<String>>(
        id: S,
        tool_result: ToolResult<CallToolResult>,
        metadata: Option<&ProviderMetadata>,
    ) -> Self {
        MessageContentBlock::ToolResponse(ToolResponse {
            id: id.into(),
            tool_result: sanitize_tool_result(tool_result),
            metadata: metadata.cloned(),
        })
    }

    pub fn action_required<S: Into<String>>(
        id: S,
        tool_name: String,
        arguments: JsonObject,
        prompt: Option<String>,
    ) -> Self {
        MessageContentBlock::ActionRequired(ActionRequired {
            data: ActionRequiredData::ToolConfirmation {
                id: id.into(),
                tool_name,
                arguments,
                prompt,
            },
        })
    }

    pub fn action_required_elicitation<S: Into<String>>(
        id: S,
        message: String,
        requested_schema: serde_json::Value,
    ) -> Self {
        MessageContentBlock::ActionRequired(ActionRequired {
            data: ActionRequiredData::Elicitation {
                id: id.into(),
                message,
                requested_schema,
            },
        })
    }

    pub fn action_required_tool_confirmation_response<S: Into<String>>(
        id: S,
        permission: crate::permission::Permission,
    ) -> Self {
        MessageContentBlock::ActionRequired(ActionRequired {
            data: ActionRequiredData::ToolConfirmationResponse {
                id: id.into(),
                permission,
            },
        })
    }

    pub fn action_required_elicitation_response<S: Into<String>>(
        id: S,
        user_data: serde_json::Value,
        action: ElicitationAction,
    ) -> Self {
        MessageContentBlock::ActionRequired(ActionRequired {
            data: ActionRequiredData::ElicitationResponse {
                id: id.into(),
                user_data,
                action,
            },
        })
    }

    pub fn thinking<S1: Into<String>, S2: Into<String>>(thinking: S1, signature: S2) -> Self {
        MessageContentBlock::Thinking(ThinkingContentBlock {
            thinking: thinking.into(),
            signature: signature.into(),
        })
    }

    pub fn redacted_thinking<S: Into<String>>(data: S) -> Self {
        MessageContentBlock::RedactedThinking(RedactedThinkingContentBlock { data: data.into() })
    }

    pub fn system_notification<S: Into<String>>(
        notification_type: SystemNotificationType,
        msg: S,
    ) -> Self {
        MessageContentBlock::SystemNotification(SystemNotificationContent {
            notification_type,
            msg: msg.into(),
            data: None,
        })
    }

    pub fn system_notification_with_data<S: Into<String>>(
        notification_type: SystemNotificationType,
        msg: S,
        data: serde_json::Value,
    ) -> Self {
        MessageContentBlock::SystemNotification(SystemNotificationContent {
            notification_type,
            msg: msg.into(),
            data: Some(data),
        })
    }

    pub fn error<S: Into<String>>(kind: MessageErrorKind, message: S) -> Self {
        MessageContentBlock::Error(ErrorContent {
            kind,
            message: message.into(),
        })
    }

    pub fn as_error(&self) -> Option<&ErrorContent> {
        if let MessageContentBlock::Error(error) = self {
            Some(error)
        } else {
            None
        }
    }

    pub fn as_system_notification(&self) -> Option<&SystemNotificationContent> {
        if let MessageContentBlock::SystemNotification(ref notification) = self {
            Some(notification)
        } else {
            None
        }
    }

    pub fn as_tool_request(&self) -> Option<&ToolRequest> {
        if let MessageContentBlock::ToolRequest(ref tool_request) = self {
            Some(tool_request)
        } else {
            None
        }
    }

    pub fn as_tool_response(&self) -> Option<&ToolResponse> {
        if let MessageContentBlock::ToolResponse(ref tool_response) = self {
            Some(tool_response)
        } else {
            None
        }
    }

    pub fn as_action_required(&self) -> Option<&ActionRequired> {
        if let MessageContentBlock::ActionRequired(ref action_required) = self {
            Some(action_required)
        } else {
            None
        }
    }

    pub fn as_tool_response_text(&self) -> Option<String> {
        if let Some(tool_response) = self.as_tool_response() {
            if let Ok(result) = &tool_response.tool_result {
                let texts: Vec<String> = result
                    .content
                    .iter()
                    .filter_map(|content| content.as_text().map(|t| t.text.to_string()))
                    .collect();
                if !texts.is_empty() {
                    return Some(texts.join("\n"));
                }
            }
        }
        None
    }

    /// Get the text content if this is a TextContent variant
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContentBlock::Text(text) => Some(&text.text),
            _ => None,
        }
    }

    /// Get the thinking content if this is a ThinkingContent variant
    pub fn as_thinking(&self) -> Option<&ThinkingContentBlock> {
        match self {
            MessageContentBlock::Thinking(thinking) => Some(thinking),
            _ => None,
        }
    }

    /// Get the redacted thinking content if this is a RedactedThinkingContent variant
    pub fn as_redacted_thinking(&self) -> Option<&RedactedThinkingContentBlock> {
        match self {
            MessageContentBlock::RedactedThinking(redacted) => Some(redacted),
            _ => None,
        }
    }
}

impl From<ContentBlock> for MessageContentBlock {
    fn from(content: ContentBlock) -> Self {
        match content {
            ContentBlock::Text(text) => MessageContentBlock::Text(text),
            ContentBlock::Image(image) => MessageContentBlock::Image(image),
            ContentBlock::ResourceLink(_link) => MessageContentBlock::text("[Resource link]"),
            ContentBlock::Resource(resource) => {
                MessageContentBlock::text(extract_text_from_resource(&resource.resource))
            }
            ContentBlock::Audio(_) => MessageContentBlock::text("[Audio content: not supported]"),
            _ => MessageContentBlock::text("[Unsupported content]"),
        }
    }
}

impl From<PromptMessage> for Message {
    fn from(prompt_message: PromptMessage) -> Self {
        // Create a new message with the appropriate role
        let message = match prompt_message.role {
            Role::User => Message::user(),
            Role::Assistant => Message::assistant(),
        };

        // Convert and add the content
        let content = match prompt_message.content {
            ContentBlock::Text(mut text) => {
                text.text = sanitize_unicode_tags(&text.text);
                MessageContentBlock::Text(text)
            }
            ContentBlock::Image(image) => MessageContentBlock::Image(image),
            ContentBlock::ResourceLink(_) => MessageContentBlock::text("[Resource link]"),
            ContentBlock::Resource(resource) => {
                MessageContentBlock::text(extract_text_from_resource(&resource.resource))
            }
            ContentBlock::Audio(_) => MessageContentBlock::text("[Audio content: not supported]"),
            _ => MessageContentBlock::text("[Unsupported content]"),
        };

        message.with_content(content)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InferenceMetadata {
    pub provider: String,
    pub requested_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MessageUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_source: Option<CostSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_compaction: bool,
}

impl MessageUsage {
    pub fn from_provider_usage(usage: &ProviderUsage, is_compaction: bool) -> Self {
        let stats = usage.stats.as_ref();
        MessageUsage {
            input_tokens: usage.usage.input_tokens,
            output_tokens: usage.usage.output_tokens,
            total_tokens: usage.usage.total_tokens,
            cache_read_tokens: usage.usage.cache_read_input_tokens,
            cache_write_tokens: usage.usage.cache_write_input_tokens,
            cost: usage.cost,
            cost_source: usage.cost_source,
            elapsed_ms: stats.and_then(|s| s.elapsed_ms),
            time_to_first_token_ms: stats.and_then(|s| s.time_to_first_token_ms),
            is_compaction,
        }
    }
}

/// Notes an operation left on a message, keyed by operation name. Nested rather
/// than free-form so metadata that is not shaped like notes fails to load
/// instead of being silently ignored.
pub type OperationNotes =
    std::collections::BTreeMap<String, serde_json::Map<String, serde_json::Value>>;

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
/// Metadata for message visibility and model inference details
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    /// Whether the message should be visible to the user in the UI
    pub user_visible: bool,
    /// Whether the message should be included in the agent's context window
    pub agent_visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference: Option<InferenceMetadata>,
    /// Whether the provider stopped generating because it reached its output token limit
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_token_limit_reached: bool,
    /// Whether this message is a steer injected into an active run. UI-only:
    /// surfaced as `_meta.goose.steer` so clients can mark the steer boundary
    /// without matching user-visible text. Never sent to providers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub steer: bool,
    /// Whether this message is a per-turn context event appended by the agent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub turn_context: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<MessageUsage>>,
    /// What an operation did to this message, keyed by operation name. Read back
    /// from the persisted conversation so a rebuilt pipeline knows what already
    /// happened. Never sent to providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Box<OperationNotes>>,
}

impl Default for MessageMetadata {
    fn default() -> Self {
        MessageMetadata {
            user_visible: true,
            agent_visible: true,
            inference: None,
            output_token_limit_reached: false,
            steer: false,
            turn_context: false,
            usage: None,
            operations: None,
        }
    }
}

impl MessageMetadata {
    pub fn operation_note(&self, operation: &str, key: &str) -> Option<&serde_json::Value> {
        self.operations.as_ref()?.get(operation)?.get(key)
    }

    pub fn set_operation_note(&mut self, operation: &str, key: &str, value: serde_json::Value) {
        self.operations
            .get_or_insert_with(Box::default)
            .entry(operation.to_string())
            .or_default()
            .insert(key.to_string(), value);
    }

    /// Create metadata for messages visible only to the agent
    pub fn agent_only() -> Self {
        MessageMetadata {
            user_visible: false,
            agent_visible: true,
            ..Default::default()
        }
    }

    /// Create metadata for messages visible only to the user
    pub fn user_only() -> Self {
        MessageMetadata {
            user_visible: true,
            agent_visible: false,
            ..Default::default()
        }
    }

    /// Create metadata for messages visible to neither user nor agent (archived)
    pub fn invisible() -> Self {
        MessageMetadata {
            user_visible: false,
            agent_visible: false,
            ..Default::default()
        }
    }

    /// Return a copy with agent_visible set to false
    pub fn with_agent_invisible(self) -> Self {
        Self {
            agent_visible: false,
            ..self
        }
    }

    /// Return a copy with user_visible set to false
    pub fn with_user_invisible(self) -> Self {
        Self {
            user_visible: false,
            ..self
        }
    }

    /// Return a copy with agent_visible set to true
    pub fn with_agent_visible(self) -> Self {
        Self {
            agent_visible: true,
            ..self
        }
    }

    /// Return a copy with user_visible set to true
    pub fn with_user_visible(self) -> Self {
        Self {
            user_visible: true,
            ..self
        }
    }

    pub fn with_inference(mut self, inference: InferenceMetadata) -> Self {
        self.inference = Some(inference);
        self
    }

    pub fn with_steer(mut self) -> Self {
        self.steer = true;
        self
    }

    pub fn with_turn_context(mut self) -> Self {
        self.turn_context = true;
        self
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
/// A message to or from an LLM
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: Option<String>,
    pub role: Role,
    pub created: i64,
    #[serde(deserialize_with = "deserialize_sanitized_content")]
    pub content: Vec<MessageContentBlock>,
    pub metadata: MessageMetadata,
}

impl Message {
    pub fn new(role: Role, created: i64, content: Vec<MessageContentBlock>) -> Self {
        Message {
            id: None,
            role,
            created,
            content,
            metadata: MessageMetadata::default(),
        }
    }
    pub fn debug(&self) -> String {
        format!("{:?}", self)
    }

    pub fn agent_visible_content(&self) -> Message {
        let filtered_content = self
            .content
            .iter()
            .filter_map(|c| c.filter_for_audience(Role::Assistant))
            .collect();

        Message {
            content: filtered_content,
            ..self.clone()
        }
    }

    pub fn user_visible_content(&self) -> Message {
        let mut filtered_content: Vec<MessageContent> = Vec::new();
        for content in self
            .content
            .iter()
            .filter_map(MessageContentBlock::user_visible_content)
        {
            match (filtered_content.last_mut(), content) {
                (
                    Some(MessageContentBlock::Text(last_text)),
                    MessageContentBlock::Text(new_text),
                ) if last_text
                    .annotations
                    .as_ref()
                    .and_then(|a| a.audience.as_ref())
                    == new_text
                        .annotations
                        .as_ref()
                        .and_then(|a| a.audience.as_ref()) =>
                {
                    last_text.text.push_str(&new_text.text);
                }
                (_, content) => filtered_content.push(content),
            }
        }

        Message {
            content: filtered_content,
            ..self.clone()
        }
    }

    /// Create a new user message with the current timestamp
    pub fn user() -> Self {
        Message {
            id: None,
            role: Role::User,
            created: Utc::now().timestamp(),
            content: Vec::new(),
            metadata: MessageMetadata::default(),
        }
    }

    /// Create a new assistant message with the current timestamp
    pub fn assistant() -> Self {
        Message {
            id: None,
            role: Role::Assistant,
            created: Utc::now().timestamp(),
            content: Vec::new(),
            metadata: MessageMetadata::default(),
        }
    }

    pub fn with_id<S: Into<String>>(mut self, id: S) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_generated_id(self) -> Self {
        self.with_id(format!("msg_{}", Uuid::new_v4()))
    }

    pub fn with_generated_id_if_missing(self) -> Self {
        if self.id.is_some() {
            self
        } else {
            self.with_generated_id()
        }
    }

    /// Add any MessageContent to the message
    pub fn with_content(mut self, content: MessageContentBlock) -> Self {
        self.content.push(content);
        self
    }

    /// Add text content to the message
    pub fn with_text<S: Into<String>>(self, text: S) -> Self {
        let raw_text = text.into();
        let sanitized_text = sanitize_unicode_tags(&raw_text);

        self.with_content(MessageContentBlock::Text(TextContent::new(sanitized_text)))
    }

    /// Add image content to the message
    pub fn with_image<S: Into<String>, T: Into<String>>(self, data: S, mime_type: T) -> Self {
        self.with_content(MessageContentBlock::image(data, mime_type))
    }

    pub fn with_document<S: Into<String>, T: Into<String>>(
        self,
        data: S,
        mime_type: T,
        name: Option<String>,
    ) -> Self {
        self.with_content(MessageContentBlock::document(data, mime_type, name))
    }

    /// Add a tool request to the message
    pub fn with_tool_request<S: Into<String>>(
        self,
        id: S,
        tool_call: ToolResult<CallToolRequestParams>,
    ) -> Self {
        self.with_content(MessageContentBlock::tool_request(id, tool_call))
    }

    pub fn with_tool_request_with_metadata<S: Into<String>>(
        self,
        id: S,
        tool_call: ToolResult<CallToolRequestParams>,
        metadata: Option<&ProviderMetadata>,
        tool_meta: Option<serde_json::Value>,
    ) -> Self {
        self.with_content(MessageContentBlock::ToolRequest(ToolRequest {
            id: id.into(),
            tool_call,
            metadata: metadata.cloned(),
            tool_meta,
        }))
    }

    pub fn with_tool_response<S: Into<String>>(
        self,
        id: S,
        result: ToolResult<CallToolResult>,
    ) -> Self {
        self.with_content(MessageContentBlock::tool_response(id, result))
    }

    pub fn add_tool_response_with_metadata<S: Into<String>>(
        &mut self,
        id: S,
        result: ToolResult<CallToolResult>,
        metadata: Option<&ProviderMetadata>,
    ) {
        self.content
            .push(MessageContentBlock::tool_response_with_metadata(
                id, result, metadata,
            ));
    }

    /// Add an action required message for tool confirmation
    pub fn with_action_required<S: Into<String>>(
        self,
        id: S,
        tool_name: String,
        arguments: JsonObject,
        prompt: Option<String>,
    ) -> Self {
        self.with_content(MessageContentBlock::action_required(
            id, tool_name, arguments, prompt,
        ))
    }

    /// Add thinking content to the message
    pub fn with_thinking<S1: Into<String>, S2: Into<String>>(
        self,
        thinking: S1,
        signature: S2,
    ) -> Self {
        self.with_content(MessageContentBlock::thinking(thinking, signature))
    }

    /// Add redacted thinking content to the message
    pub fn with_redacted_thinking<S: Into<String>>(self, data: S) -> Self {
        self.with_content(MessageContentBlock::redacted_thinking(data))
    }

    /// Get the concatenated text content of the message, separated by newlines
    pub fn as_concat_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.as_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if the message is a tool call
    pub fn is_tool_call(&self) -> bool {
        self.content
            .iter()
            .any(|c| matches!(c, MessageContentBlock::ToolRequest(_)))
    }

    /// Check if the message is a tool response
    pub fn is_tool_response(&self) -> bool {
        self.content
            .iter()
            .any(|c| matches!(c, MessageContentBlock::ToolResponse(_)))
    }

    /// Retrieves all tool `id` from the message
    pub fn get_tool_ids(&self) -> HashSet<&str> {
        self.content
            .iter()
            .filter_map(|content| match content {
                MessageContentBlock::ToolRequest(req) => Some(req.id.as_str()),
                MessageContentBlock::ToolResponse(res) => Some(res.id.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Retrieves all tool `id` from ToolRequest messages
    pub fn get_tool_request_ids(&self) -> HashSet<&str> {
        self.content
            .iter()
            .filter_map(|content| {
                if let MessageContentBlock::ToolRequest(req) = content {
                    Some(req.id.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Retrieves all tool `id` from ToolResponse messages
    pub fn get_tool_response_ids(&self) -> HashSet<&str> {
        self.content
            .iter()
            .filter_map(|content| {
                if let MessageContentBlock::ToolResponse(res) = content {
                    Some(res.id.as_str())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if the message has only TextContent
    pub fn has_only_text_content(&self) -> bool {
        self.content
            .iter()
            .all(|c| matches!(c, MessageContentBlock::Text(_)))
    }

    pub fn with_system_notification<S: Into<String>>(
        self,
        notification_type: SystemNotificationType,
        msg: S,
    ) -> Self {
        self.with_content(MessageContentBlock::system_notification(
            notification_type,
            msg,
        ))
        .with_metadata(MessageMetadata::user_only())
    }

    pub fn with_system_notification_with_data<S: Into<String>>(
        self,
        notification_type: SystemNotificationType,
        msg: S,
        data: serde_json::Value,
    ) -> Self {
        self.with_content(MessageContentBlock::system_notification_with_data(
            notification_type,
            msg,
            data,
        ))
        .with_metadata(MessageMetadata::user_only())
    }

    pub fn with_error<S: Into<String>>(self, kind: MessageErrorKind, message: S) -> Self {
        self.with_content(MessageContentBlock::error(kind, message))
            .with_metadata(MessageMetadata::user_only())
    }

    pub fn from_provider_error(err: &crate::errors::ProviderError) -> Self {
        use crate::errors::ProviderError;
        let text = match err {
            ProviderError::NetworkError(_) => {
                format!("{err}\n\nPlease resend your message to try again.")
            }
            ProviderError::ContextLengthExceeded(_) => {
                format!("{err}\n\nThe conversation is too long for the model's context window.")
            }
            _ => format!(
                "Ran into this error: {err}.\n\n\
                 Please retry if you think this is a transient or recoverable error."
            ),
        };
        Message::assistant().with_error(MessageErrorKind::from(err), text)
    }

    pub fn error_kind(&self) -> Option<MessageErrorKind> {
        self.content
            .iter()
            .find_map(|content| content.as_error().map(|error| error.kind))
    }

    pub fn with_visibility(mut self, user_visible: bool, agent_visible: bool) -> Self {
        self.metadata.user_visible = user_visible;
        self.metadata.agent_visible = agent_visible;
        self
    }

    pub fn with_metadata(mut self, metadata: MessageMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_inference(mut self, inference: InferenceMetadata) -> Self {
        self.metadata = self.metadata.with_inference(inference);
        self
    }

    pub fn with_steer(mut self) -> Self {
        self.metadata.steer = true;
        self
    }

    pub fn with_inference_if_assistant(self, inference: InferenceMetadata) -> Self {
        if self.role == Role::Assistant && self.metadata.inference.is_none() {
            self.with_inference(inference)
        } else {
            self
        }
    }

    pub fn user_only(mut self) -> Self {
        self.metadata.user_visible = true;
        self.metadata.agent_visible = false;
        self
    }

    pub fn agent_only(mut self) -> Self {
        self.metadata.user_visible = false;
        self.metadata.agent_visible = true;
        self
    }

    pub fn is_user_visible(&self) -> bool {
        self.metadata.user_visible
    }

    pub fn is_agent_visible(&self) -> bool {
        self.metadata.agent_visible
    }

    pub fn is_turn_context(&self) -> bool {
        self.metadata.turn_context
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenState {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    #[serde(default)]
    pub cache_read_tokens: i32,
    #[serde(default)]
    pub cache_write_tokens: i32,
    pub accumulated_input_tokens: i32,
    pub accumulated_output_tokens: i32,
    pub accumulated_total_tokens: i32,
    #[serde(default)]
    pub accumulated_cache_read_tokens: i32,
    #[serde(default)]
    pub accumulated_cache_write_tokens: i32,
    pub accumulated_cost: Option<f64>,
}

#[cfg(test)]
mod document_tests {
    use super::*;

    #[test]
    fn document_content_carries_media_type_and_name() {
        let message = Message::user().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        );

        let MessageContent::Document(document) = &message.content[0] else {
            panic!("expected document content");
        };
        assert_eq!(document.data, "cGRmLWJ5dGVz");
        assert_eq!(document.mime_type, "application/pdf");
        assert_eq!(document.name.as_deref(), Some("q3-report.pdf"));
        assert_eq!(
            message.content[0].to_string(),
            "[Document: q3-report.pdf (application/pdf)]"
        );
    }

    #[test]
    fn document_content_without_name_is_allowed() {
        let content = MessageContent::document("cGRmLWJ5dGVz", "application/pdf", None);

        assert!(matches!(&content, MessageContent::Document(document) if document.name.is_none()));
        assert_eq!(content.to_string(), "[Document: application/pdf]");
    }

    #[test]
    fn document_content_round_trips_through_serde() {
        let message = Message::user().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        );

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["content"][0]["type"], "document");
        assert_eq!(json["content"][0]["mimeType"], "application/pdf");
        assert_eq!(json["content"][0]["name"], "q3-report.pdf");

        let restored: Message = serde_json::from_value(json).unwrap();
        assert_eq!(restored, message);
    }
}

#[cfg(test)]
mod tests {
    use crate::conversation::message::{
        ActionRequiredData, Message, MessageContentBlock, MessageErrorKind, MessageMetadata,
        ProviderMetadata, ToolResponse,
    };
    use crate::errors::ProviderError;
    use base64::Engine;
    use rmcp::model::{
        Annotations, CallToolResult, ElicitationAction, ErrorCode, ErrorData, ImageContent,
        ResourceContents, TextContent,
    };
    use rmcp::model::{CallToolRequestParams, ContentBlock, EmbeddedResource, PromptMessage, Role};
    use rmcp::object;
    use serde_json::Value;

    #[test]
    fn provider_authentication_error_has_authentication_kind() {
        let message = Message::from_provider_error(&ProviderError::Authentication(
            "Authentication required".to_string(),
        ));

        assert_eq!(message.error_kind(), Some(MessageErrorKind::Authentication));
        assert!(message.is_user_visible());
        assert!(!message.is_agent_visible());
    }

    #[test]
    fn test_sanitize_with_text() {
        let malicious = "Hello\u{E0041}\u{E0042}\u{E0043}world"; // Invisible "ABC"
        let message = Message::user().with_text(malicious);
        assert_eq!(message.as_concat_text(), "Helloworld");
    }

    #[test]
    fn test_no_sanitize_with_text() {
        let clean_text = "Hello world 世界 🌍";
        let message = Message::user().with_text(clean_text);
        assert_eq!(message.as_concat_text(), clean_text);
    }

    #[test]
    fn test_tool_response_sanitizes_unicode_tags() {
        let content = MessageContentBlock::tool_response(
            "tool-1",
            Ok(CallToolResult::success(vec![ContentBlock::text(
                "visible\u{E0041}\u{E0042}text",
            )])),
        );

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let result = response.tool_result.unwrap();
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text content");
        };
        assert_eq!(text.text, "visibletext");
    }

    #[test]
    fn test_tool_response_with_metadata_sanitizes_unicode_tags() {
        let mut metadata = ProviderMetadata::new();
        metadata.insert("provider".to_string(), serde_json::json!("test"));
        let tagged = ContentBlock::Text(
            TextContent::new("result\u{E0041}")
                .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
        );
        let mut message = Message::user();

        message.add_tool_response_with_metadata(
            "tool-1",
            Ok(CallToolResult::success(vec![tagged])),
            Some(&metadata),
        );

        let MessageContentBlock::ToolResponse(response) = &message.content[0] else {
            panic!("expected tool response");
        };
        assert_eq!(response.metadata.as_ref(), Some(&metadata));
        let result = response.tool_result.as_ref().unwrap();
        let text = &result.content[0];
        let ContentBlock::Text(text) = text else {
            panic!("expected text content");
        };
        assert_eq!(
            text.annotations
                .as_ref()
                .and_then(|value| value.audience.as_ref()),
            Some(&vec![Role::Assistant])
        );
        assert_eq!(text.text, "result");
    }

    #[test]
    fn test_tool_response_sanitizes_error_message() {
        let data = serde_json::json!({"retry": false});
        let content = MessageContentBlock::tool_response(
            "tool-1",
            Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "error\u{E0041}text",
                Some(data.clone()),
            )),
        );

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let error = response.tool_result.unwrap_err();
        assert_eq!(error.message, "errortext");
        assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(error.data, Some(data));
    }

    #[test]
    fn test_tool_response_sanitizes_text_resource() {
        let resource = ResourceContents::TextResourceContents {
            uri: "file:///result.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "resource\u{E0041}text".to_string(),
            meta: None,
        };
        let content = MessageContentBlock::tool_response(
            "tool-1",
            Ok(CallToolResult::success(vec![ContentBlock::Resource(
                EmbeddedResource::new(resource),
            )])),
        );

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let result = response.tool_result.unwrap();
        let ContentBlock::Resource(resource) = &result.content[0] else {
            panic!("expected resource content");
        };
        let ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            meta,
        } = &resource.resource
        else {
            panic!("expected text resource");
        };
        assert_eq!(uri, "file:///result.txt");
        assert_eq!(mime_type.as_deref(), Some("text/plain"));
        assert_eq!(text, "resourcetext");
        assert!(meta.is_none());
    }

    #[test]
    fn test_tool_response_sanitizes_utf8_blob_resource() {
        let blob =
            base64::engine::general_purpose::STANDARD.encode("resource\u{E0041}text".as_bytes());
        let resource = ResourceContents::BlobResourceContents {
            uri: "file:///result.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            blob,
            meta: None,
        };
        let content = MessageContentBlock::tool_response(
            "tool-1",
            Ok(CallToolResult::success(vec![ContentBlock::Resource(
                EmbeddedResource::new(resource),
            )])),
        );

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let result = response.tool_result.unwrap();
        let ContentBlock::Resource(resource) = &result.content[0] else {
            panic!("expected resource content");
        };
        let ResourceContents::BlobResourceContents { blob, .. } = &resource.resource else {
            panic!("expected blob resource");
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(blob)
                .unwrap(),
            b"resourcetext"
        );
    }

    #[test]
    fn test_tool_response_sanitizes_malformed_blob_resource() {
        let resource = ResourceContents::BlobResourceContents {
            uri: "file:///result.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            blob: "malformed\u{E0041}text".to_string(),
            meta: None,
        };
        let content = MessageContentBlock::tool_response(
            "tool-1",
            Ok(CallToolResult::success(vec![ContentBlock::Resource(
                EmbeddedResource::new(resource),
            )])),
        );

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let result = response.tool_result.unwrap();
        let ContentBlock::Resource(resource) = &result.content[0] else {
            panic!("expected resource content");
        };
        let ResourceContents::BlobResourceContents { blob, .. } = &resource.resource else {
            panic!("expected blob resource");
        };
        assert_eq!(blob, "malformedtext");
    }

    #[test]
    fn test_deserialization_sanitizes_persisted_tool_response() {
        let message = Message::new(
            Role::User,
            1,
            vec![MessageContentBlock::ToolResponse(ToolResponse {
                id: "tool-1".to_string(),
                tool_result: Ok(CallToolResult::success(vec![ContentBlock::text(
                    "persisted\u{E0041}text",
                )])),
                metadata: None,
            })],
        );

        let json = serde_json::to_string(&message).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        let MessageContentBlock::ToolResponse(response) = &deserialized.content[0] else {
            panic!("expected tool response");
        };
        let result = response.tool_result.as_ref().unwrap();
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text content");
        };
        assert_eq!(text.text, "persistedtext");
    }

    #[test]
    fn test_content_deserialization_sanitizes_persisted_tool_response() {
        let content = vec![MessageContentBlock::ToolResponse(ToolResponse {
            id: "tool-1".to_string(),
            tool_result: Ok(CallToolResult::success(vec![ContentBlock::text(
                "persisted\u{E0041}text",
            )])),
            metadata: None,
        })];

        let json = serde_json::to_string(&content).unwrap();
        let deserialized: Vec<MessageContentBlock> = serde_json::from_str(&json).unwrap();
        let MessageContentBlock::ToolResponse(response) = &deserialized[0] else {
            panic!("expected tool response");
        };
        let result = response.tool_result.as_ref().unwrap();
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text content");
        };
        assert_eq!(text.text, "persistedtext");
    }

    #[test]
    fn test_tool_response_sanitization_preserves_legitimate_content() {
        let text = ContentBlock::Text(
            TextContent::new("世界 🌍 café")
                .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
        );
        let image = ContentBlock::Image(
            ImageContent::new("image-data", "image/png")
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let mut result = CallToolResult::success(vec![text, image]);
        result.structured_content = Some(serde_json::json!({"safe": "世界"}));
        result.meta = Some(rmcp::model::MetaObject(object!({"source": "test"})));
        let expected = result.clone();

        let content = MessageContentBlock::tool_response("tool-1", Ok(result));

        let MessageContentBlock::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        assert_eq!(response.tool_result.unwrap(), expected);
    }

    #[test]
    fn test_message_serialization() {
        let message = Message::assistant()
            .with_text("Hello, I'll help you with that.")
            .with_tool_request(
                "tool123",
                Ok(CallToolRequestParams::new("test_tool")
                    .with_arguments(object!({"param": "value"}))),
            );

        let json_str = serde_json::to_string_pretty(&message).unwrap();
        println!("Serialized message: {}", json_str);

        // Parse back to Value to check structure
        let value: Value = serde_json::from_str(&json_str).unwrap();

        // Check top-level fields
        assert_eq!(value["role"], "assistant");
        assert!(value["created"].is_i64());
        assert!(value["content"].is_array());

        // Check content items
        let content = &value["content"];

        // First item should be text
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello, I'll help you with that.");

        // Second item should be toolRequest
        assert_eq!(content[1]["type"], "toolRequest");
        assert_eq!(content[1]["id"], "tool123");

        // Check tool_call serialization
        assert_eq!(content[1]["toolCall"]["status"], "success");
        assert_eq!(content[1]["toolCall"]["value"]["name"], "test_tool");
        assert_eq!(
            content[1]["toolCall"]["value"]["arguments"]["param"],
            "value"
        );
    }

    #[test]
    fn test_error_serialization() {
        let message = Message::assistant().with_tool_request(
            "tool123",
            Err(ErrorData {
                code: ErrorCode::INTERNAL_ERROR,
                message: std::borrow::Cow::from("Something went wrong".to_string()),
                data: None,
            }),
        );

        let json_str = serde_json::to_string_pretty(&message).unwrap();
        println!("Serialized error: {}", json_str);

        // Parse back to Value to check structure
        let value: Value = serde_json::from_str(&json_str).unwrap();

        // Check tool_call serialization with error
        let tool_call = &value["content"][0]["toolCall"];
        assert_eq!(tool_call["status"], "error");
        assert_eq!(tool_call["error"], "-32603: Something went wrong");
    }

    #[test]
    fn test_deserialization() {
        // Create a JSON string with our new format
        let json_str = r#"{
            "role": "assistant",
            "created": 1740171566,
            "content": [
                {
                    "type": "text",
                    "text": "I'll help you with that."
                },
                {
                    "type": "toolRequest",
                    "id": "tool123",
                    "toolCall": {
                        "status": "success",
                        "value": {
                            "name": "test_tool",
                            "arguments": {"param": "value"}
                        }
                    }
                }
            ],
            "metadata": { "agentVisible": true, "userVisible": true }
        }"#;

        let message: Message = serde_json::from_str(json_str).unwrap();

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.created, 1740171566);
        assert_eq!(message.content.len(), 2);

        // Check first content item
        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "I'll help you with that.");
        } else {
            panic!("Expected Text content");
        }

        // Check second content item
        if let MessageContentBlock::ToolRequest(req) = &message.content[1] {
            assert_eq!(req.id, "tool123");
            if let Ok(tool_call) = &req.tool_call {
                assert_eq!(tool_call.name, "test_tool");
                assert_eq!(tool_call.arguments, Some(object!({"param": "value"})))
            } else {
                panic!("Expected successful tool call");
            }
        } else {
            panic!("Expected ToolRequest content");
        }
    }

    #[test]
    fn test_deserialization_migrates_reasoning_to_thinking() {
        let json = serde_json::json!({
            "role": "assistant",
            "created": 1740171566,
            "content": [
                { "type": "reasoning", "text": "step by step" },
                { "type": "text", "text": "final answer" }
            ],
            "metadata": { "agentVisible": true, "userVisible": true }
        });

        let message: Message = serde_json::from_value(json).unwrap();
        assert_eq!(message.content.len(), 2);

        let MessageContentBlock::Thinking(thinking) = &message.content[0] else {
            panic!("Expected Thinking content");
        };
        assert_eq!(thinking.thinking, "step by step");
        assert!(thinking.signature.is_empty());
    }

    #[test]
    fn test_elicitation_response_defaults_action_to_accept() {
        let action_required: ActionRequiredData = serde_json::from_value(serde_json::json!({
            "actionType": "elicitationResponse",
            "id": "request-123",
            "user_data": { "name": "goose" }
        }))
        .unwrap();

        let ActionRequiredData::ElicitationResponse {
            id,
            user_data,
            action,
        } = action_required
        else {
            panic!("Expected elicitation response");
        };

        assert_eq!(id, "request-123");
        assert_eq!(user_data, serde_json::json!({ "name": "goose" }));
        assert_eq!(action, ElicitationAction::Accept);
    }

    #[test]
    fn test_agent_visible_content_preserves_thinking_for_provider() {
        let message = Message::assistant()
            .with_thinking("internal reasoning", "sig")
            .with_redacted_thinking("redacted")
            .with_text("final answer");

        let provider_message = message.agent_visible_content();
        assert_eq!(provider_message.content.len(), 3);
        assert!(matches!(
            provider_message.content[0],
            MessageContentBlock::Thinking(_)
        ));
        assert!(matches!(
            provider_message.content[1],
            MessageContentBlock::RedactedThinking(_)
        ));
    }

    #[test]
    fn test_user_visible_content_filters_audience_without_dropping_thinking() {
        let assistant_text = TextContent::new("assistant text")
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant]));
        let assistant_image = ImageContent::new("assistant image", "image/png")
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant]));
        let assistant_tool_content = ContentBlock::Text(
            TextContent::new("assistant tool result")
                .with_annotations(Annotations::default().with_audience(vec![Role::Assistant])),
        );
        let user_tool_content = ContentBlock::Text(
            TextContent::new("user tool result")
                .with_annotations(Annotations::default().with_audience(vec![Role::User])),
        );
        let message = Message::assistant()
            .with_content(MessageContentBlock::Text(assistant_text))
            .with_text("shared text")
            .with_content(MessageContentBlock::Image(assistant_image))
            .with_tool_response(
                "tool-1",
                Ok(CallToolResult::success(vec![
                    assistant_tool_content,
                    user_tool_content,
                ])),
            )
            .with_thinking("visible reasoning", "sig");

        let projected = message.user_visible_content();

        assert_eq!(projected.as_concat_text(), "shared text");
        assert!(projected
            .content
            .iter()
            .any(|content| matches!(content, MessageContentBlock::Thinking(_))));
        assert!(!projected
            .content
            .iter()
            .any(|content| matches!(content, MessageContentBlock::Image(_))));
        let tool_response = projected
            .content
            .iter()
            .find_map(|content| match content {
                MessageContentBlock::ToolResponse(response) => Some(response),
                _ => None,
            })
            .expect("tool response should be preserved");
        let result = tool_response
            .tool_result
            .as_ref()
            .expect("tool result should be valid");
        assert_eq!(result.content.len(), 1);
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "user tool result"
        );
    }

    #[test]
    fn test_user_visible_content_rejoins_text_across_hidden_blocks() {
        let user_text = |text: &str| {
            MessageContentBlock::Text(
                TextContent::new(text)
                    .with_annotations(Annotations::default().with_audience(vec![Role::User])),
            )
        };
        let assistant_text = TextContent::new("provider state")
            .with_annotations(Annotations::default().with_audience(vec![Role::Assistant]));
        let message = Message::assistant()
            .with_content(user_text("Hello"))
            .with_content(MessageContentBlock::Text(assistant_text))
            .with_content(user_text(" world"));

        let projected = message.user_visible_content();

        assert_eq!(projected.content.len(), 1);
        assert_eq!(projected.as_concat_text(), "Hello world");
    }

    #[test]
    fn test_deserialization_drops_invalid_reasoning_blocks() {
        let json = serde_json::json!({
            "role": "assistant",
            "created": 1740171566,
            "content": [
                { "type": "reasoning" },
                { "type": "reasoning", "text": 42 },
                { "type": "text", "text": "still here" }
            ],
            "metadata": { "agentVisible": true, "userVisible": true }
        });

        let message: Message = serde_json::from_value(json).unwrap();
        assert_eq!(message.content.len(), 1);

        let MessageContentBlock::Text(text) = &message.content[0] else {
            panic!("Expected Text content");
        };
        assert_eq!(text.text, "still here");
    }

    #[test]
    fn test_from_prompt_message_text() {
        let prompt_content = ContentBlock::text("Hello, world!");

        let prompt_message = PromptMessage::new(Role::User, prompt_content);

        let message = Message::from(prompt_message);

        if let MessageContentBlock::Text(text_content) = &message.content[0] {
            assert_eq!(text_content.text, "Hello, world!");
        } else {
            panic!("Expected MessageContentBlock::Text");
        }
    }

    #[test]
    fn test_from_prompt_message_text_preserves_visible_unicode() {
        let prompt_message = PromptMessage::new(Role::User, ContentBlock::text("Grüße 你好 🪿"));

        let message = Message::from(prompt_message);

        assert_eq!(message.as_concat_text(), "Grüße 你好 🪿");
    }

    #[test]
    fn test_from_prompt_message_text_removes_unicode_tags() {
        let prompt_message = PromptMessage::new(
            Role::User,
            ContentBlock::text("visible\u{E0000}\u{E0041}\u{E007F} text"),
        );

        let message = Message::from(prompt_message);

        assert_eq!(message.as_concat_text(), "visible text");
    }

    #[test]
    fn test_from_prompt_message_image() {
        let prompt_content = ContentBlock::image("base64data", "image/jpeg");

        let prompt_message = PromptMessage::new(Role::User, prompt_content);

        let message = Message::from(prompt_message);

        if let MessageContentBlock::Image(image_content) = &message.content[0] {
            assert_eq!(image_content.data, "base64data");
            assert_eq!(image_content.mime_type, "image/jpeg");
        } else {
            panic!("Expected MessageContentBlock::Image");
        }
    }

    #[test]
    fn test_from_prompt_message_text_resource() {
        let resource = ResourceContents::TextResourceContents {
            uri: "file:///test.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            text: "Resource content".to_string(),
            meta: None,
        };

        let prompt_content = ContentBlock::resource(resource);

        let prompt_message = PromptMessage::new(Role::User, prompt_content);

        let message = Message::from(prompt_message);

        if let MessageContentBlock::Text(text_content) = &message.content[0] {
            assert_eq!(text_content.text, "Resource content");
        } else {
            panic!("Expected MessageContentBlock::Text");
        }
    }

    #[test]
    fn test_from_prompt_message() {
        // Test user message conversion
        let prompt_message = PromptMessage::new(Role::User, ContentBlock::text("Hello, world!"));

        let message = Message::from(prompt_message);
        assert_eq!(message.role, Role::User);
        assert_eq!(message.content.len(), 1);
        assert_eq!(message.as_concat_text(), "Hello, world!");

        // Test assistant message conversion
        let prompt_message =
            PromptMessage::new(Role::Assistant, ContentBlock::text("I can help with that."));

        let message = Message::from(prompt_message);
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.len(), 1);
        assert_eq!(message.as_concat_text(), "I can help with that.");
    }

    #[test]
    fn test_message_with_text() {
        let message = Message::user().with_text("Hello");
        assert_eq!(message.as_concat_text(), "Hello");
    }

    #[test]
    fn test_message_with_tool_request() {
        let tool_call = Ok(CallToolRequestParams::new("test_tool").with_arguments(object!({})));

        let message = Message::assistant().with_tool_request("req1", tool_call);
        assert!(message.is_tool_call());
        assert!(!message.is_tool_response());

        let ids = message.get_tool_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains("req1"));
    }

    #[test]
    fn test_message_deserialization_sanitizes_text_content() {
        // Create a test string with Unicode Tags characters
        let malicious_text = "Hello\u{E0041}\u{E0042}\u{E0043}world";
        let malicious_json = format!(
            r#"{{
            "id": "test-id",
            "role": "user",
            "created": 1640995200,
            "content": [
                {{
                    "type": "text",
                    "text": "{}"
                }},
                {{
                    "type": "image",
                    "data": "base64data",
                    "mimeType": "image/png"
                }}
            ],
            "metadata": {{ "agentVisible": true, "userVisible": true }}
        }}"#,
            malicious_text
        );

        let message: Message = serde_json::from_str(&malicious_json).unwrap();

        // Text content should be sanitized
        assert_eq!(message.as_concat_text(), "Helloworld");

        // Image content should be unchanged
        if let MessageContentBlock::Image(img) = &message.content[1] {
            assert_eq!(img.data, "base64data");
            assert_eq!(img.mime_type, "image/png");
        } else {
            panic!("Expected ImageContent");
        }
    }

    #[test]
    fn test_legitimate_unicode_preserved_during_message_deserialization() {
        let clean_json = r#"{
            "id": "test-id",
            "role": "user",
            "created": 1640995200,
            "content": [{
                "type": "text",
                "text": "Hello world 世界 🌍"
            }],
            "metadata": { "agentVisible": true, "userVisible": true }
        }"#;

        let message: Message = serde_json::from_str(clean_json).unwrap();

        assert_eq!(message.as_concat_text(), "Hello world 世界 🌍");
    }

    #[test]
    fn test_message_metadata_defaults() {
        let message = Message::user().with_text("Test");

        // By default, messages should be both user and agent visible
        assert!(message.is_user_visible());
        assert!(message.is_agent_visible());
    }

    #[test]
    fn test_message_visibility_methods() {
        // Test user_only
        let user_only_msg = Message::user().with_text("User only").user_only();
        assert!(user_only_msg.is_user_visible());
        assert!(!user_only_msg.is_agent_visible());

        // Test agent_only
        let agent_only_msg = Message::assistant().with_text("Agent only").agent_only();
        assert!(!agent_only_msg.is_user_visible());
        assert!(agent_only_msg.is_agent_visible());

        // Test with_visibility
        let custom_msg = Message::user()
            .with_text("Custom visibility")
            .with_visibility(false, true);
        assert!(!custom_msg.is_user_visible());
        assert!(custom_msg.is_agent_visible());
    }

    #[test]
    fn test_message_metadata_serialization() {
        let mut message = Message::user()
            .with_text("Test message")
            .with_visibility(false, true);
        message.metadata.output_token_limit_reached = true;

        let json_str = serde_json::to_string(&message).unwrap();
        let value: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(value["metadata"]["userVisible"], false);
        assert_eq!(value["metadata"]["agentVisible"], true);
        assert_eq!(value["metadata"]["outputTokenLimitReached"], true);
    }

    #[test]
    fn test_message_metadata_deserialization() {
        // Test with explicit metadata
        let json_with_metadata = r#"{
            "role": "user",
            "created": 1640995200,
            "content": [{
                "type": "text",
                "text": "Test"
            }],
            "metadata": {
                "userVisible": false,
                "agentVisible": true
            }
        }"#;

        let message: Message = serde_json::from_str(json_with_metadata).unwrap();
        assert!(!message.is_user_visible());
        assert!(message.is_agent_visible());
        assert!(!message.metadata.output_token_limit_reached);
    }

    #[test]
    fn test_message_metadata_static_methods() {
        // Test MessageMetadata::agent_only()
        let agent_only_metadata = MessageMetadata::agent_only();
        assert!(!agent_only_metadata.user_visible);
        assert!(agent_only_metadata.agent_visible);

        // Test MessageMetadata::user_only()
        let user_only_metadata = MessageMetadata::user_only();
        assert!(user_only_metadata.user_visible);
        assert!(!user_only_metadata.agent_visible);

        // Test MessageMetadata::invisible()
        let invisible_metadata = MessageMetadata::invisible();
        assert!(!invisible_metadata.user_visible);
        assert!(!invisible_metadata.agent_visible);

        // Test using them with messages
        let agent_msg = Message::assistant()
            .with_text("Agent only message")
            .with_metadata(MessageMetadata::agent_only());
        assert!(!agent_msg.is_user_visible());
        assert!(agent_msg.is_agent_visible());

        let user_msg = Message::user()
            .with_text("User only message")
            .with_metadata(MessageMetadata::user_only());
        assert!(user_msg.is_user_visible());
        assert!(!user_msg.is_agent_visible());

        let invisible_msg = Message::user()
            .with_text("Invisible message")
            .with_metadata(MessageMetadata::invisible());
        assert!(!invisible_msg.is_user_visible());
        assert!(!invisible_msg.is_agent_visible());
    }

    #[test]
    fn test_message_metadata_builder_methods() {
        // Test with_agent_invisible
        let metadata = MessageMetadata::default().with_agent_invisible();
        assert!(metadata.user_visible);
        assert!(!metadata.agent_visible);

        // Test with_user_invisible
        let metadata = MessageMetadata::default().with_user_invisible();
        assert!(!metadata.user_visible);
        assert!(metadata.agent_visible);

        // Test with_agent_visible
        let metadata = MessageMetadata::invisible().with_agent_visible();
        assert!(!metadata.user_visible);
        assert!(metadata.agent_visible);

        // Test with_user_visible
        let metadata = MessageMetadata::invisible().with_user_visible();
        assert!(metadata.user_visible);
        assert!(!metadata.agent_visible);

        // Test chaining
        let metadata = MessageMetadata::invisible()
            .with_user_visible()
            .with_agent_visible();
        assert!(metadata.user_visible);
        assert!(metadata.agent_visible);
    }

    #[test]
    fn test_legacy_tool_response_deserialization() {
        let legacy_json = r#"{
            "role": "user",
            "created": 1640995200,
            "content": [{
                "type": "toolResponse",
                "id": "tool123",
                "toolResult": {
                    "status": "success",
                    "value": [
                        {
                            "type": "text",
                            "text": "Tool output text"
                        }
                    ]
                }
            }],
            "metadata": { "agentVisible": true, "userVisible": true }
        }"#;

        let message: Message = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(message.content.len(), 1);

        if let MessageContentBlock::ToolResponse(response) = &message.content[0] {
            assert_eq!(response.id, "tool123");
            if let Ok(result) = &response.tool_result {
                assert_eq!(result.content.len(), 1);
                assert_eq!(
                    result.content[0].as_text().unwrap().text,
                    "Tool output text"
                );
            } else {
                panic!("Expected successful tool result");
            }
        } else {
            panic!("Expected ToolResponse content");
        }
    }

    #[test]
    fn test_new_tool_response_deserialization() {
        let new_json = r#"{
            "role": "user",
            "created": 1640995200,
            "content": [{
                "type": "toolResponse",
                "id": "tool456",
                "toolResult": {
                    "status": "success",
                    "value": {
                        "content": [
                            {
                                "type": "text",
                                "text": "New format output"
                            }
                        ],
                        "isError": false
                    }
                }
            }],
            "metadata": { "agentVisible": true, "userVisible": true }
        }"#;

        let message: Message = serde_json::from_str(new_json).unwrap();
        assert_eq!(message.content.len(), 1);

        if let MessageContentBlock::ToolResponse(response) = &message.content[0] {
            assert_eq!(response.id, "tool456");
            if let Ok(result) = &response.tool_result {
                assert_eq!(result.content.len(), 1);
                assert_eq!(
                    result.content[0].as_text().unwrap().text,
                    "New format output"
                );
            } else {
                panic!("Expected successful tool result");
            }
        } else {
            panic!("Expected ToolResponse content");
        }
    }

    #[test]
    fn test_tool_request_with_value_arguments_backward_compatibility() {
        struct TestCase {
            name: &'static str,
            arguments_json: &'static str,
            expected: Option<Value>,
        }

        let test_cases = [
            TestCase {
                name: "string",
                arguments_json: r#""string_argument""#,
                expected: Some(serde_json::json!({"value": "string_argument"})),
            },
            TestCase {
                name: "array",
                arguments_json: r#"["a", "b", "c"]"#,
                expected: Some(serde_json::json!({"value": ["a", "b", "c"]})),
            },
            TestCase {
                name: "number",
                arguments_json: "42",
                expected: Some(serde_json::json!({"value": 42})),
            },
            TestCase {
                name: "null",
                arguments_json: "null",
                expected: None,
            },
            TestCase {
                name: "object",
                arguments_json: r#"{"key": "value", "number": 123}"#,
                expected: Some(serde_json::json!({"key": "value", "number": 123})),
            },
        ];

        for tc in test_cases {
            let json = format!(
                r#"{{
                    "role": "assistant",
                    "created": 1640995200,
                    "content": [{{
                        "type": "toolRequest",
                        "id": "tool123",
                        "toolCall": {{
                            "status": "success",
                            "value": {{
                                "name": "test_tool",
                                "arguments": {}
                            }}
                        }}
                    }}],
                    "metadata": {{ "agentVisible": true, "userVisible": true }}
                }}"#,
                tc.arguments_json
            );

            let message: Message = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{}: parse failed: {}", tc.name, e));

            let MessageContentBlock::ToolRequest(request) = &message.content[0] else {
                panic!("{}: expected ToolRequest content", tc.name);
            };

            let Ok(tool_call) = &request.tool_call else {
                panic!("{}: expected successful tool call", tc.name);
            };

            assert_eq!(tool_call.name, "test_tool", "{}: wrong tool name", tc.name);

            match (&tool_call.arguments, &tc.expected) {
                (None, None) => {}
                (Some(args), Some(expected)) => {
                    let args_value = serde_json::to_value(args).unwrap();
                    assert_eq!(&args_value, expected, "{}: arguments mismatch", tc.name);
                }
                (actual, expected) => {
                    panic!("{}: expected {:?}, got {:?}", tc.name, expected, actual);
                }
            }
        }
    }
}
