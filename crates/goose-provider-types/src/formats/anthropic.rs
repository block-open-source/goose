use crate::canonical::maybe_get_canonical_model;
use crate::canonical::ThinkingMode;
use crate::conversation::message::{Message, MessageContentBlock};
use crate::conversation::token_usage::{CostSource, ProviderUsage, Usage};
use crate::documents::{
    convert_document, document_media_type_is_supported, unsupported_document_text, DocumentFormat,
    ASSISTANT_ROLE_REASON, UNSUPPORTED_MEDIA_TYPE_REASON,
};
use crate::errors::ProviderError;
use crate::images::{convert_image, ImageFormat};
use crate::mcp_utils::extract_text_from_resource;
use crate::model::ModelConfig;
use crate::thinking::ThinkingEffort;
use anyhow::{anyhow, Result};
use rmcp::model::{
    object, CallToolRequestParams, ContentBlock, ErrorCode, ErrorData, JsonObject,
    ResourceContents, Role, Tool,
};
use rmcp::object as json_object;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

pub const ANTHROPIC_PROVIDER_NAME: &str = "anthropic";

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $str:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.to_lowercase().as_str() {
                    $($str => Ok(Self::$variant),)+
                    other => Err(format!("unknown {}: '{other}'", stringify!($name))),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self { $(Self::$variant => write!(f, $str),)+ }
            }
        }
    }
}

string_enum!(ThinkingType { Adaptive => "adaptive", Enabled => "enabled", Disabled => "disabled" });
string_enum!(CacheTtl { FiveMinutes => "5m", OneHour => "1h" });

string_enum!(PrefixMismatchBehavior { DropBlock => "drop_block", Error => "error" });

pub const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";
pub const INPUT_TRANSFORMATIONS_FIELD: &str = "input_transformations";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnthropicFormatOptions {
    pub preserve_unsigned_thinking: bool,
    pub preserve_thinking_context: bool,
    pub thinking_disabled: bool,
    pub emit_clear_thinking: bool,
    pub current_model: Option<String>,
    pub prompt_cache_disabled: bool,
    pub cache_ttl: Option<CacheTtl>,
    pub prefix_mismatch_behavior: Option<PrefixMismatchBehavior>,
    pub strip_thinking_history: bool,
}

impl AnthropicFormatOptions {
    /// Anthropic-compatible providers keep `Default`, which does not request block binding.
    pub fn native() -> Self {
        Self {
            prefix_mismatch_behavior: Some(PrefixMismatchBehavior::DropBlock),
            ..Self::default()
        }
    }

    fn for_model(self, model_config: &ModelConfig) -> Self {
        let preserve_thinking_context = model_config
            .request_param::<bool>("preserve_thinking_context")
            .unwrap_or(self.preserve_thinking_context);
        let preserve_unsigned_thinking = model_config
            .request_param::<bool>("preserve_unsigned_thinking")
            .unwrap_or(self.preserve_unsigned_thinking)
            || preserve_thinking_context;
        let thinking_disabled = model_config.reasoning == Some(false)
            || model_config.thinking_effort() == Some(ThinkingEffort::Off);
        let emit_clear_thinking = model_config
            .request_param::<bool>("emit_clear_thinking")
            .unwrap_or(self.emit_clear_thinking);
        let cache_ttl = model_config
            .cache_ttl()
            .and_then(|ttl| ttl.parse::<CacheTtl>().ok())
            .or(self.cache_ttl);
        let prefix_mismatch_behavior = match model_config
            .request_param::<String>("prefix_mismatch_behavior")
            .as_deref()
        {
            None => self.prefix_mismatch_behavior,
            Some("off") => None,
            Some(value) => value.parse().ok().or(self.prefix_mismatch_behavior),
        };

        Self {
            preserve_unsigned_thinking,
            preserve_thinking_context,
            thinking_disabled,
            emit_clear_thinking,
            current_model: self
                .current_model
                .or_else(|| Some(model_config.model_name.clone())),
            prompt_cache_disabled: model_config.prompt_cache_disabled(),
            cache_ttl,
            prefix_mismatch_behavior,
            strip_thinking_history: self.strip_thinking_history,
        }
    }

    /// `{"type":"ephemeral"}` selects Anthropic's default 5m TTL; the `ttl`
    /// field is only sent for an explicit 1h opt-in, since a 1h write is
    /// billed at 2x input instead of 1.25x.
    fn cache_control(&self) -> Value {
        match self.cache_ttl {
            Some(CacheTtl::OneHour) => {
                json!({ TYPE_FIELD: "ephemeral", "ttl": "1h" })
            }
            _ => json!({ TYPE_FIELD: "ephemeral" }),
        }
    }
}

pub fn thinking_block_is_stale(message: &Message, current_model: Option<&str>) -> bool {
    let Some(current_model) = current_model else {
        return false;
    };
    let Some(inference) = message.metadata.inference.as_ref() else {
        return false;
    };
    let requested = inference.requested_model.as_str();
    let resolved = inference.resolved_model.as_deref().unwrap_or("");
    if requested.is_empty() && resolved.is_empty() {
        return false;
    }
    current_model != requested && current_model != resolved
}

fn canonical_thinking_mode(provider_name: &str, model_name: &str) -> Option<ThinkingMode> {
    maybe_get_canonical_model(provider_name, model_name).and_then(|model| model.thinking_mode)
}

/// Adaptive models run adaptive thinking when `thinking` is omitted, so turning
/// it off takes an explicit disable. Always-on models reject that disable.
pub fn requires_explicit_thinking_disable(provider_name: &str, model_name: &str) -> bool {
    canonical_thinking_mode(provider_name, model_name) == Some(ThinkingMode::Adaptive)
}

fn canonical_reasoning(provider_name: &str, model_config: &ModelConfig) -> Option<bool> {
    maybe_get_canonical_model(provider_name, &model_config.model_name)
        .and_then(|model| model.reasoning)
}

pub fn model_supports_temperature(provider_name: &str, model_config: &ModelConfig) -> bool {
    maybe_get_canonical_model(provider_name, &model_config.model_name)
        .and_then(|model| model.temperature)
        .unwrap_or(true)
}

pub fn thinking_type(model_config: &ModelConfig) -> ThinkingType {
    thinking_type_for_provider(ANTHROPIC_PROVIDER_NAME, model_config)
}

pub fn thinking_type_for_provider(provider_name: &str, model_config: &ModelConfig) -> ThinkingType {
    let mode = canonical_thinking_mode(provider_name, &model_config.model_name);
    let reasoning = model_config
        .reasoning
        .or_else(|| canonical_reasoning(provider_name, model_config));

    if reasoning != Some(true) {
        return ThinkingType::Disabled;
    }

    if mode == Some(ThinkingMode::AlwaysOnAdaptive) {
        return ThinkingType::Adaptive;
    }

    let effort = model_config.thinking_effort();

    if effort.is_none() && model_config.request_param::<i32>("budget_tokens").is_some() {
        return match mode {
            Some(ThinkingMode::Adaptive) => ThinkingType::Adaptive,
            _ => ThinkingType::Enabled,
        };
    }

    match effort.unwrap_or(ThinkingEffort::Off) {
        ThinkingEffort::Off => ThinkingType::Disabled,
        _ if mode == Some(ThinkingMode::Adaptive) => ThinkingType::Adaptive,
        _ => ThinkingType::Enabled,
    }
}

// Constants for frequently used strings in Anthropic API format
const TYPE_FIELD: &str = "type";
const CONTENT_FIELD: &str = "content";
const TEXT_TYPE: &str = "text";
const ROLE_FIELD: &str = "role";
const USER_ROLE: &str = "user";
const ASSISTANT_ROLE: &str = "assistant";
const TOOL_USE_TYPE: &str = "tool_use";
const TOOL_RESULT_TYPE: &str = "tool_result";
const THINKING_TYPE: &str = "thinking";
const REDACTED_THINKING_TYPE: &str = "redacted_thinking";
const CACHE_CONTROL_FIELD: &str = "cache_control";
const ID_FIELD: &str = "id";
const NAME_FIELD: &str = "name";
const INPUT_FIELD: &str = "input";
const TOOL_USE_ID_FIELD: &str = "tool_use_id";
const IS_ERROR_FIELD: &str = "is_error";
const SIGNATURE_FIELD: &str = "signature";
const DATA_FIELD: &str = "data";
const IMAGE_TYPE: &str = "image";
const DOCUMENT_TYPE: &str = "document";
const SOURCE_FIELD: &str = "source";
const BASE64_TYPE: &str = "base64";
const MEDIA_TYPE_FIELD: &str = "media_type";
// Claude vision only accepts these image media types; other image/* blobs fall
// through to the text/binary-marker path so an unsupported type (e.g.
// image/svg+xml) doesn't turn the next request into a provider rejection.
const ANTHROPIC_IMAGE_MEDIA_TYPES: [&str; 4] =
    ["image/jpeg", "image/png", "image/gif", "image/webp"];
const EVENT_MESSAGE_START: &str = "message_start";
const EVENT_MESSAGE_DELTA: &str = "message_delta";
const EVENT_MESSAGE_STOP: &str = "message_stop";
const EVENT_CONTENT_BLOCK_START: &str = "content_block_start";
const EVENT_CONTENT_BLOCK_DELTA: &str = "content_block_delta";
const EVENT_CONTENT_BLOCK_STOP: &str = "content_block_stop";
const STOP_REASON_REFUSAL: &str = "refusal";
const REFUSAL_FALLBACK_DETAILS: &str = "No additional details were provided.";

/// Coerce a tool call's optional arguments into the JSON value Anthropic
/// expects for the `input` field of a `tool_use` content block.
///
/// Anthropic's Messages API requires `input` to be an object. When the
/// internal `CallToolRequestParams::arguments` is `None` (which happens for
/// parameterless tools, tool calls round-tripped from disk, or calls created
/// via `CallToolRequestParams::new` without `.with_arguments(...)`) the
/// `json!` macro would otherwise serialize it as JSON `null` and the API
/// rejects the next replay of the tool_use block with a 400 error:
/// `messages.<N>.content.<M>.tool_use.input: Input should be an object.`
/// See issue #9287.
fn args_to_input_value(arguments: Option<JsonObject>) -> Value {
    Value::Object(arguments.unwrap_or_default())
}

/// Convert internal Message format to Anthropic's API message specification
pub fn format_messages(messages: &[Message]) -> Vec<Value> {
    format_messages_with_options(messages, &AnthropicFormatOptions::default())
}

fn format_messages_with_options(
    messages: &[Message],
    options: &AnthropicFormatOptions,
) -> Vec<Value> {
    let mut anthropic_messages = Vec::new();

    for message in messages {
        let role = match message.role {
            Role::User => USER_ROLE,
            Role::Assistant => ASSISTANT_ROLE,
        };

        let thinking_is_stale = thinking_block_is_stale(message, options.current_model.as_deref());
        let replay_thinking =
            !options.thinking_disabled && !options.strip_thinking_history && !thinking_is_stale;

        let mut content = Vec::new();
        for msg_content in &message.content {
            match msg_content {
                MessageContentBlock::Text(text) => {
                    if !text.text.trim().is_empty() {
                        content.push(json!({
                            TYPE_FIELD: TEXT_TYPE,
                            TEXT_TYPE: text.text
                        }));
                    }
                }
                MessageContentBlock::ToolRequest(tool_request) => {
                    match &tool_request.tool_call {
                        Ok(tool_call) => {
                            content.push(json!({
                                TYPE_FIELD: TOOL_USE_TYPE,
                                ID_FIELD: tool_request.id,
                                NAME_FIELD: tool_call.name,
                                INPUT_FIELD: args_to_input_value(tool_call.arguments.clone())
                            }));
                        }
                        Err(_tool_error) => {
                            // The paired tool response carries the parse error and
                            // serializes to a tool_result below; Anthropic rejects a
                            // tool_result without a preceding tool_use, so emit a
                            // placeholder tool_use with the same id to keep history valid.
                            content.push(json!({
                                TYPE_FIELD: TOOL_USE_TYPE,
                                ID_FIELD: tool_request.id,
                                NAME_FIELD: "unparseable_tool_call",
                                INPUT_FIELD: json!({})
                            }));
                        }
                    }
                }
                MessageContentBlock::ToolResponse(tool_response) => {
                    match &tool_response.tool_result {
                        Ok(result) => {
                            let mut blocks: Vec<Value> = Vec::new();
                            let mut text_parts: Vec<String> = Vec::new();
                            let mut has_media = false;

                            for c in result.content.iter() {
                                if let Some(t) = c.as_text() {
                                    text_parts.push(t.text.clone());
                                    if !t.text.is_empty() {
                                        blocks.push(json!({
                                            TYPE_FIELD: TEXT_TYPE,
                                            TEXT_TYPE: t.text.clone()
                                        }));
                                    }
                                    continue;
                                }
                                if let Some(r) = c.as_resource() {
                                    // Claude only accepts a fixed set of media types, so
                                    // unsupported blobs fall back to text below rather than
                                    // being rejected by the provider.
                                    if let ResourceContents::BlobResourceContents {
                                        blob,
                                        mime_type,
                                        ..
                                    } = &r.resource
                                    {
                                        let mime = mime_type.as_deref().unwrap_or("");
                                        if ANTHROPIC_IMAGE_MEDIA_TYPES.contains(&mime) {
                                            has_media = true;
                                            blocks.push(json!({
                                                TYPE_FIELD: IMAGE_TYPE,
                                                SOURCE_FIELD: {
                                                    TYPE_FIELD: BASE64_TYPE,
                                                    MEDIA_TYPE_FIELD: mime,
                                                    DATA_FIELD: blob,
                                                }
                                            }));
                                            continue;
                                        }
                                        if mime == "application/pdf" {
                                            has_media = true;
                                            blocks.push(json!({
                                                TYPE_FIELD: DOCUMENT_TYPE,
                                                SOURCE_FIELD: {
                                                    TYPE_FIELD: BASE64_TYPE,
                                                    MEDIA_TYPE_FIELD: mime,
                                                    DATA_FIELD: blob,
                                                }
                                            }));
                                            continue;
                                        }
                                    }
                                    let text = extract_text_from_resource(&r.resource);
                                    if !text.is_empty() {
                                        text_parts.push(text.clone());
                                        blocks.push(json!({
                                            TYPE_FIELD: TEXT_TYPE,
                                            TEXT_TYPE: text
                                        }));
                                    }
                                    continue;
                                }
                                if let ContentBlock::Image(image) = c {
                                    if ANTHROPIC_IMAGE_MEDIA_TYPES
                                        .contains(&image.mime_type.as_str())
                                    {
                                        has_media = true;
                                        blocks.push(convert_image(
                                            &image.clone(),
                                            &ImageFormat::Anthropic,
                                        ));
                                    } else {
                                        let marker = format!("[Image: {}]", image.mime_type);
                                        text_parts.push(marker.clone());
                                        blocks.push(json!({
                                            TYPE_FIELD: TEXT_TYPE,
                                            TEXT_TYPE: marker
                                        }));
                                    }
                                }
                            }

                            let content_value = if has_media {
                                Value::Array(blocks)
                            } else {
                                Value::String(text_parts.join("\n"))
                            };

                            content.push(json!({
                                TYPE_FIELD: TOOL_RESULT_TYPE,
                                TOOL_USE_ID_FIELD: tool_response.id,
                                CONTENT_FIELD: content_value
                            }));
                        }
                        Err(tool_error) => {
                            content.push(json!({
                                TYPE_FIELD: TOOL_RESULT_TYPE,
                                TOOL_USE_ID_FIELD: tool_response.id,
                                CONTENT_FIELD: format!("Error: {}", tool_error),
                                IS_ERROR_FIELD: true
                            }));
                        }
                    }
                }
                MessageContentBlock::ToolConfirmationRequest(_tool_confirmation_request) => {
                    // Skip tool confirmation requests
                }
                MessageContentBlock::ActionRequired(_action_required) => {
                    // Skip action required messages - they're for UI only
                }
                MessageContentBlock::SystemNotification(_) | MessageContentBlock::Error(_) => {
                    // Skip
                }
                MessageContentBlock::Thinking(thinking) => {
                    // Anthropic rejects thinking blocks sent without a matching thinking config.
                    if replay_thinking {
                        if !thinking.signature.is_empty() {
                            content.push(json!({
                                TYPE_FIELD: THINKING_TYPE,
                                THINKING_TYPE: thinking.thinking,
                                SIGNATURE_FIELD: thinking.signature
                            }));
                        } else if options.preserve_unsigned_thinking
                            && !thinking.thinking.is_empty()
                        {
                            content.push(json!({
                                TYPE_FIELD: THINKING_TYPE,
                                THINKING_TYPE: thinking.thinking
                            }));
                        }
                    }
                }
                MessageContentBlock::RedactedThinking(redacted) => {
                    if replay_thinking {
                        content.push(json!({
                            TYPE_FIELD: REDACTED_THINKING_TYPE,
                            DATA_FIELD: redacted.data
                        }));
                    }
                }
                MessageContentBlock::Image(image) => {
                    content.push(convert_image(image, &ImageFormat::Anthropic));
                }
                MessageContentBlock::Document(document) => {
                    if message.role != Role::User {
                        content.push(json!({
                            TYPE_FIELD: TEXT_TYPE,
                            TEXT_TYPE: unsupported_document_text(document, ASSISTANT_ROLE_REASON)
                        }));
                    } else if document_media_type_is_supported(&document.mime_type) {
                        content.push(convert_document(document, &DocumentFormat::Anthropic));
                    } else {
                        content.push(json!({
                            TYPE_FIELD: TEXT_TYPE,
                            TEXT_TYPE: unsupported_document_text(
                                document,
                                UNSUPPORTED_MEDIA_TYPE_REASON,
                            )
                        }));
                    }
                }
            }
        }

        // Skip messages with empty content
        if !content.is_empty() {
            anthropic_messages.push(json!({
                ROLE_FIELD: role,
                CONTENT_FIELD: content
            }));
        }
    }

    if anthropic_messages.is_empty() {
        anthropic_messages.push(json!({
            ROLE_FIELD: USER_ROLE,
            CONTENT_FIELD: [{
                TYPE_FIELD: TEXT_TYPE,
                TEXT_TYPE: "Ignore"
            }]
        }));
    }

    if options.prompt_cache_disabled {
        return anthropic_messages;
    }

    // The last two user messages extend the cached prefix each turn.
    let mut user_count = 0;
    for message in anthropic_messages.iter_mut().rev() {
        if message.get(ROLE_FIELD) != Some(&json!(USER_ROLE)) {
            continue;
        }
        if let Some(block) = message
            .get_mut(CONTENT_FIELD)
            .and_then(|content| content.as_array_mut())
            .and_then(|content_array| content_array.last_mut())
            .and_then(|b| b.as_object_mut())
        {
            block.insert(CACHE_CONTROL_FIELD.to_string(), options.cache_control());
            user_count += 1;
            if user_count >= 2 {
                break;
            }
        }
    }

    anthropic_messages
}

fn anthropic_flavored_input_schema(input_schema: Arc<JsonObject>) -> Arc<JsonObject> {
    if input_schema.is_empty() {
        return Arc::new(json_object!({
            "type": "object",
        }));
    }
    input_schema
}

/// Convert internal Tool format to Anthropic's API tool specification
pub fn format_tools(tools: &[Tool], options: &AnthropicFormatOptions) -> Vec<Value> {
    let mut unique_tools = HashSet::new();
    let mut tool_specs = Vec::new();

    for tool in tools {
        if unique_tools.insert(tool.name.clone()) {
            tool_specs.push(json!({
                NAME_FIELD: tool.name,
                "description": tool.description,
                "input_schema": anthropic_flavored_input_schema(tool.input_schema.clone())
            }));
        }
    }

    if options.prompt_cache_disabled {
        return tool_specs;
    }

    // Add "cache_control" to the last tool spec, if any. This means that all tool definitions,
    // will be cached as a single prefix.
    if let Some(last_tool) = tool_specs.last_mut() {
        last_tool
            .as_object_mut()
            .unwrap()
            .insert(CACHE_CONTROL_FIELD.to_string(), options.cache_control());
    }

    tool_specs
}

/// Convert system message to Anthropic's API system specification
pub fn format_system(system: &str, options: &AnthropicFormatOptions) -> Value {
    if options.prompt_cache_disabled {
        return json!([{
            TYPE_FIELD: TEXT_TYPE,
            TEXT_TYPE: system
        }]);
    }
    json!([{
        TYPE_FIELD: TEXT_TYPE,
        TEXT_TYPE: system,
        CACHE_CONTROL_FIELD: options.cache_control()
    }])
}

/// Convert Anthropic's API response to internal Message format
pub fn response_to_message(response: &Value) -> Result<Message> {
    let content_blocks = response
        .get(CONTENT_FIELD)
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("Invalid response format: missing content array"))?;

    let mut message = Message::assistant();

    for block in content_blocks {
        match block.get(TYPE_FIELD).and_then(|t| t.as_str()) {
            Some(TEXT_TYPE) => {
                if let Some(text) = block.get(TEXT_TYPE).and_then(|t| t.as_str()) {
                    message = message.with_text(text.to_string());
                }
            }
            Some(TOOL_USE_TYPE) => {
                let id = block
                    .get(ID_FIELD)
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| anyhow!("Missing tool_use id"))?;
                let name = block
                    .get(NAME_FIELD)
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| anyhow!("Missing tool_use name"))?
                    .to_string();
                let input = block
                    .get(INPUT_FIELD)
                    .ok_or_else(|| anyhow!("Missing tool_use input"))?;

                let tool_call =
                    CallToolRequestParams::new(name).with_arguments(object(input.clone()));
                message = message.with_tool_request(id, Ok(tool_call));
            }
            Some(THINKING_TYPE) => {
                let thinking = block
                    .get(THINKING_TYPE)
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| anyhow!("Missing thinking content"))?
                    .to_string();
                let signature = block
                    .get(SIGNATURE_FIELD)
                    .and_then(|s| s.as_str())
                    .unwrap_or_default();
                message = message.with_thinking(thinking, signature);
            }
            Some(REDACTED_THINKING_TYPE) => {
                let data = block
                    .get(DATA_FIELD)
                    .and_then(|d| d.as_str())
                    .ok_or_else(|| anyhow!("Missing redacted_thinking data"))?;
                message = message.with_redacted_thinking(data);
            }
            _ => continue,
        }
    }

    Ok(message)
}

fn usage_from_anthropic_fields(usage: &Value) -> Usage {
    let field = |key: &str| {
        usage
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v.min(i32::MAX as u64) as i32)
    };

    Usage::from_cache_exclusive_input(
        Some(field("input_tokens").unwrap_or(0)),
        Some(field("output_tokens").unwrap_or(0)),
        None,
        field("cache_read_input_tokens"),
        field("cache_creation_input_tokens"),
    )
}

/// Merge a `message_delta` usage into the usage captured at `message_start`.
/// Delta usage is cumulative (input grows during server tool use), so fields
/// present in the raw delta payload win over the start values.
fn merge_delta_usage(existing: &Usage, delta: &Usage, delta_data: &Value) -> Usage {
    let reports = |key: &str| delta_data.get(key).is_some();

    let output = if reports("output_tokens") {
        delta.output_tokens
    } else {
        existing.output_tokens
    };

    if !reports("input_tokens") {
        Usage::new(existing.input_tokens, output, None).with_cache_tokens(
            existing.cache_read_input_tokens,
            existing.cache_write_input_tokens,
        )
    } else if reports("cache_read_input_tokens") || reports("cache_creation_input_tokens") {
        Usage::new(delta.input_tokens, output, None).with_cache_tokens(
            delta.cache_read_input_tokens,
            delta.cache_write_input_tokens,
        )
    } else {
        Usage::from_cache_exclusive_input(
            delta.input_tokens,
            output,
            None,
            existing.cache_read_input_tokens,
            existing.cache_write_input_tokens,
        )
    }
}

pub fn get_usage(data: &Value) -> Result<Usage> {
    if let Some(usage) = data.get("usage") {
        Ok(usage_from_anthropic_fields(usage))
    } else if data.as_object().is_some() {
        // Check if the data itself is the usage object (for message_delta events that might have usage at top level)
        let usage = usage_from_anthropic_fields(data);
        if usage.total_tokens.unwrap_or(0) > 0 {
            Ok(usage)
        } else {
            tracing::debug!("🔍 Anthropic no token data found in object");
            Ok(Usage::new(None, None, None))
        }
    } else {
        tracing::debug!(
            "Failed to get usage data: {}",
            ProviderError::UsageError("No usage data found in response".to_string())
        );
        // If no usage data, return None for all values
        Ok(Usage::new(None, None, None))
    }
}

/// Anthropic response fields that have no canonical `ProviderUsage` equivalent.
const ADDITIONAL_USAGE_FIELDS: [&str; 1] = ["service_tier"];

pub fn input_transformations(message_data: &Value) -> Option<Value> {
    let transformations = message_data.get(INPUT_TRANSFORMATIONS_FIELD)?.as_array()?;
    let dropped: Vec<(&str, &str)> = transformations
        .iter()
        .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("thinking_dropped"))
        .map(|t| {
            (
                t.get("path").and_then(Value::as_str).unwrap_or(""),
                t.get("reason").and_then(Value::as_str).unwrap_or(""),
            )
        })
        .collect();
    if !dropped.is_empty() {
        tracing::warn!(?dropped, "API dropped thinking blocks from the request");
    }
    Some(Value::Array(transformations.clone()))
}

pub fn get_additional_data(data: &Value) -> Option<Map<String, Value>> {
    let usage = data.get("usage")?.as_object()?;
    let additional: Map<String, Value> = ADDITIONAL_USAGE_FIELDS
        .iter()
        .filter_map(|field| Some(((*field).to_string(), usage.get(*field)?.clone())))
        .collect();
    (!additional.is_empty()).then_some(additional)
}

fn provider_usage_with_cost(
    model: String,
    usage: Usage,
    data: &Value,
    fallback_cost: Option<f64>,
) -> ProviderUsage {
    let provider_usage = ProviderUsage::new(model, usage);
    match super::openai::get_cost(data).or(fallback_cost) {
        Some(cost) => provider_usage.with_cost(cost, CostSource::ProviderReported),
        None => provider_usage,
    }
}

pub fn thinking_effort(model_config: &ModelConfig) -> ThinkingEffort {
    model_config
        .thinking_effort()
        .unwrap_or(ThinkingEffort::High)
}

pub fn adaptive_output_effort(model_config: &ModelConfig) -> ThinkingEffort {
    match thinking_effort(model_config) {
        ThinkingEffort::Off => ThinkingEffort::High,
        effort => effort,
    }
}

pub fn thinking_budget_tokens(model_config: &ModelConfig) -> i32 {
    if let Some(request_param) = model_config
        .request_params
        .as_ref()
        .and_then(|params| params.get("budget_tokens"))
        .and_then(|v| serde_json::from_value::<i32>(v.clone()).ok())
    {
        return request_param.max(1024);
    }

    let effort = model_config
        .thinking_effort()
        .unwrap_or(ThinkingEffort::High);
    match effort {
        ThinkingEffort::Off => 1024,
        ThinkingEffort::Low => 4000,
        ThinkingEffort::Medium => 10000,
        ThinkingEffort::High => 16000,
        ThinkingEffort::Max => 32000,
    }
}

// Anthropic counts thinking tokens against max_tokens, so the budget must leave
// room for a response. Clamp it to preserve at least this many answer tokens, and
// drop thinking only when even a minimal budget wouldn't fit under the cap.
// Shared with the Bedrock formatter, which applies the same clamp.
pub const MIN_ANSWER_TOKENS: i32 = 1024;

fn apply_thinking_config(
    payload: &mut Value,
    provider_name: &str,
    model_config: &ModelConfig,
    max_tokens: i32,
    options: AnthropicFormatOptions,
) {
    let obj = payload.as_object_mut().unwrap();
    match thinking_type_for_provider(provider_name, model_config) {
        ThinkingType::Adaptive => {
            obj.insert("thinking".to_string(), json!({"type": "adaptive"}));
            let effort = adaptive_output_effort(model_config).to_string();
            obj.insert("output_config".to_string(), json!({"effort": effort}));
        }
        ThinkingType::Enabled => {
            let budget_tokens = thinking_budget_tokens(model_config)
                .min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
            if budget_tokens >= MIN_ANSWER_TOKENS {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens
                    }),
                );
            }
        }
        ThinkingType::Disabled => {}
    }

    if options.preserve_thinking_context && !options.thinking_disabled {
        if !obj.contains_key("thinking") {
            let budget_tokens = thinking_budget_tokens(model_config)
                .min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
            if budget_tokens >= MIN_ANSWER_TOKENS {
                obj.insert(
                    "thinking".to_string(),
                    json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens
                    }),
                );
            }
        }

        // Z.AI requires this to preserve reasoning; Anthropic rejects it.
        if options.emit_clear_thinking {
            if let Some(thinking) = obj.get_mut("thinking").and_then(|t| t.as_object_mut()) {
                thinking.insert("clear_thinking".to_string(), json!(false));
            }
        }
    }

    if !obj.contains_key("thinking")
        && requires_explicit_thinking_disable(provider_name, &model_config.model_name)
    {
        obj.insert("thinking".to_string(), json!({"type": "disabled"}));
    }

    // `block_binding` is only accepted alongside adaptive or enabled thinking.
    if let Some(behavior) = options.prefix_mismatch_behavior {
        if let Some(thinking) = obj.get_mut("thinking").and_then(|t| t.as_object_mut()) {
            if thinking.get("type").and_then(|t| t.as_str()) != Some("disabled") {
                thinking.insert(
                    "block_binding".to_string(),
                    json!({"prefix_mismatch_behavior": behavior.to_string()}),
                );
            }
        }
    }
}

pub fn block_binding_behavior(payload: &Value) -> Option<PrefixMismatchBehavior> {
    payload
        .pointer("/thinking/block_binding/prefix_mismatch_behavior")
        .and_then(Value::as_str)
        .and_then(|behavior| behavior.parse().ok())
}

pub fn is_thinking_signature_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("thinking")
        && (lower.contains("signature")
            || lower.contains("cannot be modified")
            || lower.contains("block_binding"))
}

pub fn create_request(
    provider_name: &str,
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    options: AnthropicFormatOptions,
) -> Result<Value> {
    create_request_for_model(
        provider_name,
        model_config,
        &model_config.model_name,
        system,
        messages,
        tools,
        options,
    )
}

pub fn create_request_for_model(
    provider_name: &str,
    model_config: &ModelConfig,
    wire_model_name: &str,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    options: AnthropicFormatOptions,
) -> Result<Value> {
    let options = options.for_model(model_config);
    let anthropic_messages = format_messages_with_options(messages, &options);
    let tool_specs = format_tools(tools, &options);
    let system_spec = format_system(system, &options);

    if anthropic_messages.is_empty() {
        return Err(anyhow!("No valid messages to send to Anthropic API"));
    }

    let max_tokens = model_config.max_output_tokens();
    let mut payload = json!({
        "model": wire_model_name,
        "messages": anthropic_messages,
        "max_tokens": max_tokens,
    });

    if !system.is_empty() {
        payload
            .as_object_mut()
            .unwrap()
            .insert("system".to_string(), json!(system_spec));
    }

    if !tool_specs.is_empty() {
        payload
            .as_object_mut()
            .unwrap()
            .insert("tools".to_string(), json!(tool_specs));
    }

    if model_supports_temperature(provider_name, model_config) {
        if let Some(temp) = model_config.temperature {
            payload
                .as_object_mut()
                .unwrap()
                .insert("temperature".to_string(), json!(temp));
        }
    }

    apply_thinking_config(
        &mut payload,
        provider_name,
        model_config,
        max_tokens,
        options,
    );

    Ok(payload)
}

/// Process streaming response from Anthropic's API
pub fn response_to_streaming_message<S>(
    mut stream: S,
) -> impl futures::Stream<Item = anyhow::Result<(Option<Message>, Option<ProviderUsage>)>> + 'static
where
    S: futures::Stream<Item = anyhow::Result<String>> + Unpin + Send + 'static,
{
    use async_stream::try_stream;
    use futures::StreamExt;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    struct StreamingEvent {
        #[serde(rename = "type")]
        event_type: String,
        #[serde(flatten)]
        data: Value,
    }

    #[derive(Deserialize, Debug)]
    #[serde(tag = "type", rename_all = "snake_case")]
    #[allow(clippy::enum_variant_names)]
    enum ContentBlockDelta {
        TextDelta { text: String },
        InputJsonDelta { partial_json: String },
        ThinkingDelta { thinking: String },
        SignatureDelta { signature: String },
    }

    struct ThinkingState {
        text: String,
        signature: String,
    }

    fn block_index(event_data: &Value) -> Option<i32> {
        event_data
            .get("index")
            .and_then(|v| v.as_i64())
            .map(|index| index as i32)
    }

    try_stream! {
        struct StreamingToolCall {
            id: String,
            name: String,
            arguments: String,
        }

        let mut accumulated_tool_calls: std::collections::HashMap<i32, StreamingToolCall> = std::collections::HashMap::new();
        let mut final_usage: Option<ProviderUsage> = None;
        let mut message_id: Option<String> = None;
        let mut thinking: Option<ThinkingState> = None;
        let mut stop_reason: Option<String> = None;
        let mut additional_data: Option<Map<String, Value>> = None;

        while let Some(line_result) = stream.next().await {
            let line = line_result?;

            // Skip empty lines and non-data lines
            // Note: SSE spec allows both "data: value" and "data:value" (space is optional)
            if line.trim().is_empty() || !line.starts_with("data:") {
                continue;
            }

            let data_part = line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")).unwrap_or(&line);

            // Handle end of stream
            if data_part.trim() == "[DONE]" {
                break;
            }

            // Parse the JSON event
            let event: StreamingEvent = match serde_json::from_str(data_part) {
                Ok(event) => event,
                Err(e) => {
                    tracing::debug!("Failed to parse streaming event: {} - Line: {}", e, data_part);
                    continue;
                }
            };

            match event.event_type.as_str() {
                EVENT_MESSAGE_START => {
                    if let Some(message_data) = event.data.get("message") {
                        additional_data = get_additional_data(message_data);
                        if let Some(transformations) = input_transformations(message_data) {
                            additional_data
                                .get_or_insert_with(Map::new)
                                .insert(INPUT_TRANSFORMATIONS_FIELD.to_string(), transformations);
                        }
                        if let Some(id) = message_data.get("id").and_then(|v| v.as_str()) {
                            message_id = Some(id.to_string());
                        }

                        if let Some(usage_data) = message_data.get("usage") {
                            let usage = get_usage(usage_data).unwrap_or_default();
                            let model = message_data.get("model")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            final_usage = Some(provider_usage_with_cost(model, usage, usage_data, None));
                        }
                    }
                    continue;
                }
                EVENT_CONTENT_BLOCK_START => {
                    if let Some(content_block) = event.data.get("content_block") {
                        match content_block.get(TYPE_FIELD).and_then(|v| v.as_str()) {
                            Some(TOOL_USE_TYPE) => {
                                if let (Some(index), Some(id), Some(name)) = (
                                    block_index(&event.data),
                                    content_block.get("id").and_then(|v| v.as_str()),
                                    content_block.get("name").and_then(|v| v.as_str()),
                                ) {
                                    accumulated_tool_calls.insert(index, StreamingToolCall {
                                        id: id.to_string(),
                                        name: name.to_string(),
                                        arguments: String::new(),
                                    });
                                }
                            }
                            Some(THINKING_TYPE) => {
                                thinking = Some(ThinkingState {
                                    text: content_block
                                        .get(THINKING_TYPE)
                                        .and_then(|t| t.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                    signature: content_block
                                        .get(SIGNATURE_FIELD)
                                        .and_then(|s| s.as_str())
                                        .unwrap_or_default()
                                        .to_string(),
                                });
                            }
                            Some(REDACTED_THINKING_TYPE) => {
                                if let Some(data) = content_block.get(DATA_FIELD).and_then(|d| d.as_str()) {
                                    let mut message = Message::assistant()
                                        .with_redacted_thinking(data);
                                    message.id = message_id.clone();
                                    yield (Some(message), None);
                                } else {
                                    tracing::warn!("redacted_thinking block missing '{}' field", DATA_FIELD);
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                EVENT_CONTENT_BLOCK_DELTA => {
                    if let Some(delta) = event.data.get("delta") {
                        match serde_json::from_value::<ContentBlockDelta>(delta.clone()) {
                            Ok(ContentBlockDelta::TextDelta { text }) => {
                                let mut message = Message::assistant().with_text(&text);
                                message.id = message_id.clone();
                                yield (Some(message), None);
                            }
                            Ok(ContentBlockDelta::InputJsonDelta { partial_json }) => {
                                if let Some(call) = block_index(&event.data)
                                    .and_then(|index| accumulated_tool_calls.get_mut(&index))
                                {
                                    call.arguments.push_str(&partial_json);
                                }
                            }
                            Ok(ContentBlockDelta::ThinkingDelta { thinking: t }) => {
                                if let Some(ref mut state) = thinking {
                                    state.text.push_str(&t);
                                }
                            }
                            Ok(ContentBlockDelta::SignatureDelta { signature: s }) => {
                                if let Some(ref mut state) = thinking {
                                    state.signature.push_str(&s);
                                }
                            }
                            Err(e) => {
                                tracing::debug!("Unknown content_block_delta type: {}", e);
                            }
                        }
                    }
                    continue;
                }
                EVENT_CONTENT_BLOCK_STOP => {
                    if let Some(state) = thinking.take() {
                        // Omitted thinking arrives as an empty string with a signature and must still be replayed.
                        if !state.text.is_empty() || !state.signature.is_empty() {
                            let mut message = Message::assistant()
                                .with_thinking(state.text, state.signature);
                            message.id = message_id.clone();
                            yield (Some(message), None);
                        }
                    }
                    if let Some(index) = block_index(&event.data) {
                        if let Some(call) = accumulated_tool_calls.remove(&index) {
                            let StreamingToolCall { id, name, arguments } = call;
                            let parsed_args = if arguments.is_empty() {
                                json!({})
                            } else {
                                match crate::json::parse_tool_arguments(&arguments) {
                                    Some(parsed) => parsed,
                                    None => {
                                        let message_text = crate::json::truncation_error_message(&arguments)
                                            .unwrap_or_else(|| {
                                                format!("Could not parse tool arguments: {arguments}")
                                            });
                                        let error = ErrorData::new(
                                            ErrorCode::INVALID_PARAMS,
                                            message_text,
                                            None,
                                        );
                                        let mut message = Message::new(
                                            Role::Assistant,
                                            chrono::Utc::now().timestamp(),
                                            vec![MessageContentBlock::tool_request_with_provider_index(id, Err(error), None, index)],
                                        );
                                        message.id = message_id.clone();
                                        yield (Some(message), None);
                                        continue;
                                    }
                                }
                            };

                            let tool_call = CallToolRequestParams::new(name).with_arguments(object(parsed_args));

                            let mut message = Message::new(
                                rmcp::model::Role::Assistant,
                                chrono::Utc::now().timestamp(),
                                vec![MessageContentBlock::tool_request_with_provider_index(id, Ok(tool_call), None, index)],
                            );
                            message.id = message_id.clone();
                            yield (Some(message), None);
                        }
                    }
                    continue;
                }
                EVENT_MESSAGE_DELTA => {
                    if let Some(usage_data) = event.data.get("usage") {
                        let delta_usage = get_usage(usage_data).unwrap_or_default();

                        if let Some(existing_usage) = &final_usage {
                            let merged_usage = merge_delta_usage(&existing_usage.usage, &delta_usage, usage_data);
                            final_usage = Some(provider_usage_with_cost(
                                existing_usage.model.clone(),
                                merged_usage,
                                usage_data,
                                existing_usage.cost,
                            ));
                        } else {
                            let model = event.data.get("model")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            final_usage = Some(provider_usage_with_cost(model, delta_usage, usage_data, None));
                        }
                    }
                    if let Some(delta) = event.data.get("delta") {
                        let stop_details = delta.get("stop_details").filter(|d| !d.is_null());
                        if stop_reason.is_none() {
                            if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                                stop_reason = Some(sr.to_string());
                            }
                        }
                        if delta.get("stop_reason").and_then(|v| v.as_str()) == Some(STOP_REASON_REFUSAL) {
                            let str_field = |key: &str| stop_details
                                .and_then(|d| d.get(key))
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                            let details = str_field("explanation")
                                .or_else(|| stop_details.map(|d| d.to_string()))
                                .unwrap_or_else(|| REFUSAL_FALLBACK_DETAILS.to_string());
                            let category = str_field("category");
                            // The refusal delta carries the request's usage;
                            // flush it so refused turns are still accounted.
                            if let Some(mut usage) = final_usage.take() {
                                usage.finish_reasons = Some(vec![STOP_REASON_REFUSAL.to_string()]);
                                usage.response_id = message_id.clone();
                                usage.additional_data = additional_data.clone();
                                yield (None, Some(usage));
                            }
                            Err(ProviderError::Refusal { details, category })?;
                        } else if let Some(details) = stop_details {
                            // No specific handling for these stop details yet —
                            // forward them rather than silently dropping the turn.
                            let mut message = Message::assistant().with_text(format!(
                                "The provider ended the response with: {details}"
                            ));
                            message.id = message_id.clone();
                            yield (Some(message), None);
                        }
                    }
                    continue;
                }
                EVENT_MESSAGE_STOP => {
                    if let Some(usage_data) = event.data.get("usage") {
                        let usage = get_usage(usage_data).unwrap_or_default();
                        let model = event.data.get("model")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let fallback_cost = final_usage.as_ref().and_then(|u| u.cost);
                        final_usage = Some(provider_usage_with_cost(model, usage, usage_data, fallback_cost));
                    }
                    break;
                }
                _ => {
                    // Unknown event type, log and continue
                    tracing::debug!("Unknown streaming event type: {}", event.event_type);
                    continue;
                }
            }
        }

        // A tool_use block left open at stream end never received its
        // content_block_stop, so its args are truncated rather than complete.
        if !accumulated_tool_calls.is_empty() {
            let truncated_by_limit = stop_reason.as_deref() == Some("max_tokens");
            let mut indices: Vec<i32> = accumulated_tool_calls.keys().copied().collect();
            indices.sort();
            for index in indices {
                if let Some(StreamingToolCall { id, arguments: args, .. }) = accumulated_tool_calls.remove(&index) {
                    let guidance = if truncated_by_limit {
                        "The model's response was truncated — it hit the output token limit while generating this tool call. \
                         Try increasing max_tokens for this provider or breaking the task into smaller steps."
                    } else {
                        "A tool call was not completed before the stream ended. \
                         Try resending your message or breaking the task into smaller steps."
                    };
                    let snippet_len = args.chars().count();
                    let tail: String = args.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
                    let message_text = format!(
                        "{guidance}\nReceived {snippet_len} characters of arguments; cut off at: …{tail}"
                    );
                    let error = ErrorData::new(ErrorCode::INVALID_PARAMS, message_text, None);
                    let mut message = Message::new(
                        Role::Assistant,
                        chrono::Utc::now().timestamp(),
                        vec![MessageContentBlock::tool_request_with_provider_index(id, Err(error), None, index)],
                    );
                    message.id = message_id.clone();
                    yield (Some(message), None);
                }
            }
        }

        if stop_reason.as_deref() == Some("max_tokens") {
            let mut message = Message::assistant();
            message.id = message_id.clone();
            message.metadata.output_token_limit_reached = true;
            yield (Some(message), None);
        }

        if let Some(mut usage) = final_usage {
            if let Some(reason) = stop_reason {
                usage.finish_reasons = Some(vec![reason]);
            }
            if let Some(id) = message_id {
                usage.response_id = Some(id);
            }
            usage.additional_data = additional_data;
            yield (None, Some(usage));
        }
    }
}

#[cfg(test)]
mod document_tests {
    use super::*;

    #[test]
    fn user_document_becomes_a_base64_document_block() {
        let messages = vec![Message::user().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");
        let block = &spec[0]["content"][0];
        assert_eq!(block["type"], DOCUMENT_TYPE);
        assert_eq!(block["title"], "q3-report.pdf");
        assert_eq!(
            block[SOURCE_FIELD],
            json!({
                "type": "base64",
                "media_type": "application/pdf",
                "data": "cGRmLWJ5dGVz",
            })
        );
    }

    #[test]
    fn assistant_document_becomes_a_text_block() {
        let messages = vec![Message::assistant().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["content"][0]["type"], "text");
        let text = spec[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("q3-report.pdf"), "{text}");
        assert!(text.contains("user messages"), "{text}");
        assert!(!text.contains("cGRmLWJ5dGVz"), "{text}");
    }

    #[test]
    fn unsupported_document_media_type_becomes_an_explicit_text_block() {
        let messages = vec![Message::user().with_document(
            "cm93cw==",
            "text/csv",
            Some("rows.csv".to_string()),
        )];

        let spec = format_messages(&messages);

        assert_eq!(spec[0]["content"][0]["type"], "text");
        let text = spec[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("rows.csv"), "{text}");
        assert!(text.contains("text/csv"), "{text}");
        assert!(text.contains("application/pdf"), "{text}");
        assert!(!text.contains("cm93cw=="), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::{Message, MessageContent};
    use crate::model::ModelConfig;
    use rmcp::object;
    use serde_json::json;

    /// Create a complete request payload for Anthropic's API
    fn create_request_with_default_options(
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<Value> {
        create_request_with_options_provider(
            model_config,
            system,
            messages,
            tools,
            AnthropicFormatOptions::default(),
        )
    }

    fn create_request_with_options_provider(
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        options: AnthropicFormatOptions,
    ) -> Result<Value> {
        create_request(
            ANTHROPIC_PROVIDER_NAME,
            model_config,
            system,
            messages,
            tools,
            options,
        )
    }

    #[test]
    fn test_parse_text_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Hello! How can I assist you today?"
            }],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 15,
                "cache_creation_input_tokens": 12,
                "cache_read_input_tokens": 0
            }
        });

        let message = response_to_message(&response)?;
        let usage = get_usage(&response)?;

        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "Hello! How can I assist you today?");
        } else {
            panic!("Expected Text content");
        }

        assert_eq!(usage.input_tokens, Some(24)); // 12 + 12 = 24 actual tokens
        assert_eq!(usage.output_tokens, Some(15));
        assert_eq!(usage.total_tokens, Some(39)); // 24 + 15
        assert_eq!(usage.cache_read_input_tokens, Some(0));
        assert_eq!(usage.cache_write_input_tokens, Some(12));

        Ok(())
    }

    #[test]
    fn test_parse_tool_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "tool_1",
                "name": "calculator",
                "input": {
                    "expression": "2 + 2"
                }
            }],
            "model": "claude-3-sonnet-20240229",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 15,
                "output_tokens": 20,
                "cache_creation_input_tokens": 15,
                "cache_read_input_tokens": 0,
            }
        });

        let message = response_to_message(&response)?;
        let usage = get_usage(&response)?;

        if let MessageContentBlock::ToolRequest(tool_request) = &message.content[0] {
            let tool_call = tool_request.tool_call.as_ref().unwrap();
            assert_eq!(tool_call.name, "calculator");
            assert_eq!(tool_call.arguments, Some(object!({"expression": "2 + 2"})));
        } else {
            panic!("Expected ToolRequest content");
        }

        assert_eq!(usage.input_tokens, Some(30)); // 15 + 15 = 30 actual tokens
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(50)); // 30 + 20
        assert_eq!(usage.cache_read_input_tokens, Some(0));
        assert_eq!(usage.cache_write_input_tokens, Some(15));

        Ok(())
    }

    #[test]
    fn test_parse_unsigned_thinking_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "thinking",
                "thinking": "internal reasoning"
            }],
            "model": "glm-4.7",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 12,
                "output_tokens": 15
            }
        });

        let message = response_to_message(&response)?;

        if let MessageContentBlock::Thinking(thinking) = &message.content[0] {
            assert_eq!(thinking.thinking, "internal reasoning");
            assert_eq!(thinking.signature, "");
        } else {
            panic!("Expected Thinking content");
        }

        Ok(())
    }

    #[test]
    fn test_message_to_anthropic_spec() {
        let messages = vec![
            Message::user().with_text("Hello"),
            Message::assistant().with_text("Hi there"),
            Message::user().with_text("How are you?"),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 3);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(spec[0]["content"][0]["type"], "text");
        assert_eq!(spec[0]["content"][0]["text"], "Hello");
        assert_eq!(spec[1]["role"], "assistant");
        assert_eq!(spec[1]["content"][0]["text"], "Hi there");
        assert_eq!(spec[2]["role"], "user");
        assert_eq!(spec[2]["content"][0]["text"], "How are you?");
    }

    #[test]
    fn test_message_to_anthropic_spec_skips_unsigned_thinking() {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::assistant().with_text("Hi there"),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["content"][0]["type"], "text");
        assert_eq!(spec[0]["content"][0]["text"], "Hi there");
    }

    #[test]
    fn test_message_to_anthropic_spec_preserves_unsigned_thinking_when_enabled() {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::assistant().with_text("Hi there"),
        ];

        let spec = format_messages_with_options(
            &messages,
            &AnthropicFormatOptions {
                preserve_unsigned_thinking: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["content"][0]["type"], "thinking");
        assert_eq!(spec[0]["content"][0]["thinking"], "internal");
        assert!(spec[0]["content"][0].get("signature").is_none());
        assert_eq!(spec[1]["content"][0]["text"], "Hi there");
    }

    fn signed_thinking_from_model(model: &str) -> Message {
        use crate::conversation::message::InferenceMetadata;
        Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-abc"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "anthropic".to_string(),
                requested_model: model.to_string(),
                resolved_model: None,
                provider_session_id: None,
            })
    }

    #[test]
    fn strip_thinking_history_removes_signed_blocks() {
        let messages = vec![Message::assistant()
            .with_content(MessageContent::thinking("", "sig"))
            .with_text("answer")];
        let opts = AnthropicFormatOptions {
            strip_thinking_history: true,
            ..Default::default()
        };
        let spec = format_messages_with_options(&messages, &opts);
        assert_eq!(
            spec[0]["content"],
            json!([{"type": "text", "text": "answer"}])
        );
    }

    #[test]
    fn drops_signed_thinking_from_a_different_model() {
        let messages = vec![signed_thinking_from_model("claude-opus-4-1")];
        let opts = AnthropicFormatOptions {
            current_model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let spec = format_messages_with_options(&messages, &opts);
        let types: Vec<&str> = spec[0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["type"].as_str().unwrap())
            .collect();
        assert!(
            !types.contains(&"thinking"),
            "stale thinking must be dropped"
        );
        assert!(types.contains(&"text"), "text content must be preserved");
    }

    #[test]
    fn keeps_signed_thinking_from_the_same_model() {
        let messages = vec![signed_thinking_from_model("claude-sonnet-4-5")];
        let opts = AnthropicFormatOptions {
            current_model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let spec = format_messages_with_options(&messages, &opts);
        assert_eq!(spec[0]["content"][0]["type"], "thinking");
        assert_eq!(spec[0]["content"][0]["signature"], "sig-abc");
    }

    #[test]
    fn keeps_signed_thinking_when_provenance_unknown() {
        let messages =
            vec![Message::assistant().with_content(MessageContent::thinking("internal", "sig"))];
        let opts = AnthropicFormatOptions {
            current_model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let spec = format_messages_with_options(&messages, &opts);
        assert_eq!(spec[0]["content"][0]["type"], "thinking");
    }

    #[test]
    fn test_tools_to_anthropic_spec() {
        let tools = vec![
            Tool::new(
                "calculator",
                "Calculate mathematical expressions",
                object!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The mathematical expression to evaluate"
                        }
                    }
                }),
            ),
            Tool::new(
                "weather",
                "Get weather information",
                object!({
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "The location to get weather for"
                        }
                    }
                }),
            ),
        ];

        let spec = format_tools(&tools, &AnthropicFormatOptions::default());

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0]["name"], "calculator");
        assert_eq!(spec[0]["description"], "Calculate mathematical expressions");
        assert_eq!(spec[1]["name"], "weather");
        assert_eq!(spec[1]["description"], "Get weather information");

        // Verify cache control is added to last tool
        assert!(spec[1].get("cache_control").is_some());
    }

    #[test]
    fn test_system_to_anthropic_spec() {
        let system = "You are a helpful assistant.";
        let spec = format_system(system, &AnthropicFormatOptions::default());

        assert!(spec.is_array());
        let spec_array = spec.as_array().unwrap();
        assert_eq!(spec_array.len(), 1);
        assert_eq!(spec_array[0]["type"], "text");
        assert_eq!(spec_array[0]["text"], system);
        assert!(spec_array[0].get("cache_control").is_some());
    }

    #[test]
    fn test_cache_pricing_calculation() -> Result<()> {
        // Test realistic cache scenario: small fresh input, large cached content
        let response = json!({
            "id": "msg_cache_test",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Based on the cached context, here's my response."
            }],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 7,        // Small fresh input
                "output_tokens": 50,      // Output tokens
                "cache_creation_input_tokens": 10000, // Large cache creation
                "cache_read_input_tokens": 5000       // Large cache read
            }
        });

        let usage = get_usage(&response)?;

        // ACTUAL input tokens should be:
        // 7 + 10000 + 5000 = 15007 total actual tokens
        assert_eq!(usage.input_tokens, Some(15007));
        assert_eq!(usage.output_tokens, Some(50));
        assert_eq!(usage.total_tokens, Some(15057)); // 15007 + 50
        assert_eq!(usage.cache_read_input_tokens, Some(5000));
        assert_eq!(usage.cache_write_input_tokens, Some(10000));

        Ok(())
    }

    #[test]
    fn test_create_request_adaptive_thinking_for_46_models() -> Result<()> {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);

        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), json!("high"));

        let mut config = cfg("claude-opus-4-6");
        config.max_tokens = Some(4096);
        config.request_params = Some(params);
        let messages = vec![Message::user().with_text("Hello")];
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert_eq!(payload["output_config"]["effort"], "high");
        assert!(payload.get("budget_tokens").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_adaptive_thinking_for_opus_5() -> Result<()> {
        // Claude 5 models reject the legacy thinking.type=enabled shape and
        // require thinking.type=adaptive + output_config.effort.
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);

        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), json!("high"));

        let mut config = cfg("claude-opus-5");
        config.max_tokens = Some(4096);
        config.request_params = Some(params);
        let messages = vec![Message::user().with_text("Hello")];
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert_eq!(payload["output_config"]["effort"], "high");
        assert!(payload["thinking"].get("budget_tokens").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_enabled_thinking_with_budget() -> Result<()> {
        let _guard = env_lock::lock_env([
            ("GOOSE_THINKING_EFFORT", None::<&str>),
            ("ANTHROPIC_PRESERVE_THINKING_CONTEXT", None::<&str>),
        ]);

        let mut config = cfg_with_effort("claude-sonnet-4-5-20250929", "high");
        config.max_tokens = Some(64000);

        let messages = vec![Message::user().with_text("Hello")];
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"]["type"], "enabled");
        let budget = payload["thinking"]["budget_tokens"].as_i64().unwrap();
        assert!(budget > 0);
        assert_eq!(payload["max_tokens"], 64000);
        assert!(budget < 64000);

        Ok(())
    }

    #[test]
    fn test_create_request_clamps_thinking_budget_to_fit_max_tokens() -> Result<()> {
        let _guard = env_lock::lock_env([
            ("GOOSE_THINKING_EFFORT", None::<&str>),
            ("ANTHROPIC_PRESERVE_THINKING_CONTEXT", None::<&str>),
        ]);

        let mut config = cfg_with_effort("claude-sonnet-4-5-20250929", "high");
        let messages = vec![Message::user().with_text("Hello")];

        // Budget larger than max_tokens is clamped to leave room for a response.
        config.max_tokens = Some(4096);
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;
        let budget = payload["thinking"]["budget_tokens"].as_i64().unwrap();
        assert!(budget >= 1024);
        assert!(budget <= 4096 - 1024);
        assert_eq!(payload["max_tokens"], 4096);

        // Too small to fit any thinking alongside a response — drop it.
        config.max_tokens = Some(1500);
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;
        assert!(payload.get("thinking").is_none());
        assert_eq!(payload["max_tokens"], 1500);

        Ok(())
    }

    #[test]
    fn test_create_request_disabled_thinking_no_thinking_field() -> Result<()> {
        let _guard = env_lock::lock_env([
            ("GOOSE_THINKING_EFFORT", None::<&str>),
            ("ANTHROPIC_PRESERVE_THINKING_CONTEXT", None::<&str>),
        ]);

        let config = cfg_with_effort("claude-sonnet-4-20250514", "off");
        let messages = vec![Message::user().with_text("Hello")];
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert!(payload.get("thinking").is_none());
        assert!(payload.get("output_config").is_none());

        // Adaptive models treat an omitted field as adaptive, so off must be explicit.
        let config = cfg_with_effort("claude-opus-5", "off");
        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"], json!({"type": "disabled"}));

        Ok(())
    }

    #[test]
    fn test_create_request_preserves_thinking_context_for_compatible_models() -> Result<()> {
        let _guard = env_lock::lock_env([
            ("CLAUDE_THINKING_ENABLED", None::<&str>),
            ("ANTHROPIC_PRESERVE_THINKING_CONTEXT", None::<&str>),
            ("ANTHROPIC_PRESERVE_UNSIGNED_THINKING", None::<&str>),
        ]);

        let mut config = cfg("glm-4.7");
        config.max_tokens = Some(64000);
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::user().with_text("Continue"),
        ];

        let payload = create_request_with_options_provider(
            &config,
            "system",
            &messages,
            &[],
            AnthropicFormatOptions {
                preserve_unsigned_thinking: true,
                preserve_thinking_context: true,
                emit_clear_thinking: true,
                ..Default::default()
            },
        )?;

        assert_eq!(payload["thinking"]["type"], "enabled");
        assert!(payload["thinking"]["budget_tokens"].as_i64().unwrap() >= 1024);
        assert_eq!(payload["thinking"]["clear_thinking"], false);
        assert_eq!(payload["max_tokens"], 64000);
        assert_eq!(payload["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(payload["messages"][0]["content"][0]["thinking"], "internal");
        assert!(payload["messages"][0]["content"][0]
            .get("signature")
            .is_none());

        // Preserved context still wins on models that need an explicit thinking disable.
        let mut config = cfg("claude-opus-5");
        config.max_tokens = Some(64000);
        let payload = create_request_with_options_provider(
            &config,
            "system",
            &messages,
            &[],
            AnthropicFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        )?;

        assert_eq!(payload["thinking"]["type"], "enabled");

        Ok(())
    }

    #[test]
    fn test_enabled_thinking_without_optin_omits_clear_thinking() -> Result<()> {
        let mut config = cfg("claude-sonnet-4-5-20250929");
        config.max_tokens = Some(64000);
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::user().with_text("Continue"),
        ];

        let payload = create_request_with_options_provider(
            &config,
            "system",
            &messages,
            &[],
            AnthropicFormatOptions {
                preserve_unsigned_thinking: true,
                preserve_thinking_context: true,
                emit_clear_thinking: false,
                ..Default::default()
            },
        )?;

        assert_eq!(payload["thinking"]["type"], "enabled");
        assert!(
            payload["thinking"].get("clear_thinking").is_none(),
            "Anthropic enabled thinking must not carry clear_thinking, got {}",
            payload["thinking"]
        );

        Ok(())
    }

    #[test]
    fn test_create_request_model_params_enable_preserved_thinking_context() -> Result<()> {
        let _guard = env_lock::lock_env([
            ("CLAUDE_THINKING_ENABLED", None::<&str>),
            ("ANTHROPIC_PRESERVE_THINKING_CONTEXT", None::<&str>),
            ("ANTHROPIC_PRESERVE_UNSIGNED_THINKING", None::<&str>),
        ]);

        let mut params = std::collections::HashMap::new();
        params.insert("preserve_thinking_context".to_string(), json!(true));
        params.insert("emit_clear_thinking".to_string(), json!(true));

        let mut config = cfg("glm-4.7");
        config.request_params = Some(params);
        config.max_tokens = Some(64000);
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::user().with_text("Continue"),
        ];

        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"]["clear_thinking"], false);
        assert_eq!(payload["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(payload["messages"][0]["content"][0]["thinking"], "internal");

        Ok(())
    }

    #[test]
    fn test_adaptive_model_preserved_thinking_omits_clear_thinking() -> Result<()> {
        let mut config = cfg_with_effort("claude-opus-4-8", "high");
        config.max_tokens = Some(64000);
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("internal", "")),
            Message::user().with_text("Continue"),
        ];

        let payload = create_request_with_options_provider(
            &config,
            "system",
            &messages,
            &[],
            AnthropicFormatOptions {
                preserve_unsigned_thinking: true,
                preserve_thinking_context: true,
                emit_clear_thinking: false,
                ..Default::default()
            },
        )?;

        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert!(
            payload["thinking"].get("clear_thinking").is_none(),
            "adaptive thinking must not carry clear_thinking, got {}",
            payload["thinking"]
        );
        assert_eq!(payload["output_config"]["effort"], "high");

        Ok(())
    }

    #[test]
    fn test_tool_error_handling_maintains_pairing() {
        use crate::conversation::message::Message;
        use rmcp::model::{ErrorCode, ErrorData};

        let messages = vec![
            Message::assistant().with_tool_request(
                "tool_1",
                Ok(CallToolRequestParams::new("calculator")
                    .with_arguments(object!({"expression": "2 + 2"}))),
            ),
            Message::user().with_tool_response(
                "tool_1",
                Err(ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    "Tool failed".to_string(),
                    None,
                )),
            ),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 2);

        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["content"][0]["type"], "tool_use");
        assert_eq!(spec[0]["content"][0]["id"], "tool_1");
        assert_eq!(spec[0]["content"][0]["name"], "calculator");

        assert_eq!(spec[1]["role"], "user");
        assert_eq!(spec[1]["content"][0]["type"], "tool_result");
        assert_eq!(spec[1]["content"][0]["tool_use_id"], "tool_1");
        assert_eq!(
            spec[1]["content"][0]["content"],
            "Error: -32603: Tool failed"
        );
        assert_eq!(spec[1]["content"][0]["is_error"], true);
    }

    #[test]
    fn test_whitespace_only_text_blocks_are_skipped() {
        let messages = vec![
            Message::user().with_text("Hello"),
            Message::assistant().with_text("").with_tool_request(
                "tool_1",
                Ok(CallToolRequestParams::new("search").with_arguments(object!({"query": "test"}))),
            ),
            Message::user()
                .with_tool_response("tool_1", Ok(rmcp::model::CallToolResult::success(vec![]))),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 3);

        let assistant_content = spec[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "tool_use");
    }

    #[test]
    fn test_tool_response_with_resource_content() {
        use rmcp::model::{CallToolResult, ContentBlock};

        let resource_content = ContentBlock::embedded_text(
            "file:///test/file.txt",
            "This is the file content from a resource",
        );

        let messages = vec![
            Message::assistant().with_tool_request(
                "tool_1",
                Ok(CallToolRequestParams::new("view_file")
                    .with_arguments(object!({"path": "/test/file.txt"}))),
            ),
            Message::user().with_tool_response(
                "tool_1",
                Ok(CallToolResult::success(vec![resource_content])),
            ),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[1]["role"], "user");
        assert_eq!(spec[1]["content"][0]["type"], "tool_result");
        assert_eq!(spec[1]["content"][0]["tool_use_id"], "tool_1");
        assert_eq!(
            spec[1]["content"][0]["content"],
            "This is the file content from a resource"
        );
    }

    #[test]
    fn test_tool_response_with_mixed_content() {
        use rmcp::model::{CallToolResult, ContentBlock};

        let text_content = ContentBlock::text("Summary: file loaded");
        let resource_content =
            ContentBlock::embedded_text("file:///test/file.txt", "File content here");

        let messages = vec![
            Message::assistant().with_tool_request(
                "tool_1",
                Ok(CallToolRequestParams::new("view_file")
                    .with_arguments(object!({"path": "/test/file.txt"}))),
            ),
            Message::user().with_tool_response(
                "tool_1",
                Ok(CallToolResult::success(vec![
                    text_content,
                    resource_content,
                ])),
            ),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec[1]["content"][0]["type"], "tool_result");
        assert_eq!(
            spec[1]["content"][0]["content"],
            "Summary: file loaded\nFile content here"
        );
    }

    #[test]
    fn test_tool_response_forwards_image_resource_as_image_block() {
        use rmcp::model::CallToolResult;

        let image = ContentBlock::resource(ResourceContents::BlobResourceContents {
            uri: "file:///shot.png".to_string(),
            mime_type: Some("image/png".to_string()),
            blob: "aGVsbG8=".to_string(),
            meta: None,
        });

        let messages = vec![
            Message::assistant()
                .with_tool_request("tool_1", Ok(CallToolRequestParams::new("screenshot"))),
            Message::user().with_tool_response("tool_1", Ok(CallToolResult::success(vec![image]))),
        ];

        let spec = format_messages(&messages);

        let block = &spec[1]["content"][0]["content"][0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "aGVsbG8=");
    }

    #[test]
    fn test_tool_response_unsupported_image_mime_falls_back_to_text() {
        use rmcp::model::CallToolResult;

        // image/svg+xml is not a Claude-supported image type, so it must fall
        // through to text rather than an image block.
        let svg = ContentBlock::resource(ResourceContents::BlobResourceContents {
            uri: "file:///diagram.svg".to_string(),
            mime_type: Some("image/svg+xml".to_string()),
            blob: "aGVsbG8=".to_string(),
            meta: None,
        });

        let messages = vec![
            Message::assistant()
                .with_tool_request("tool_1", Ok(CallToolRequestParams::new("render"))),
            Message::user().with_tool_response("tool_1", Ok(CallToolResult::success(vec![svg]))),
        ];

        let spec = format_messages(&messages);

        // Serializer contract: an unsupported image type is not emitted as an
        // image block — the content collapses to a text string.
        assert!(spec[1]["content"][0]["content"].is_string());
    }

    #[test]
    fn test_tool_response_forwards_raw_image_as_image_block() {
        use rmcp::model::CallToolResult;

        let image = ContentBlock::image("aGVsbG8=", "image/png");

        let messages = vec![
            Message::assistant()
                .with_tool_request("tool_1", Ok(CallToolRequestParams::new("screenshot"))),
            Message::user().with_tool_response("tool_1", Ok(CallToolResult::success(vec![image]))),
        ];

        let spec = format_messages(&messages);

        let block = &spec[1]["content"][0]["content"][0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "aGVsbG8=");
    }

    #[test]
    fn test_tool_response_unsupported_raw_image_mime_falls_back_to_text() {
        use rmcp::model::CallToolResult;

        // image/svg+xml is not a Claude-supported image type, so a raw image with
        // that mime must fall back to a text marker rather than an image block
        // (which the provider would reject).
        let image = ContentBlock::image("aGVsbG8=", "image/svg+xml");

        let messages = vec![
            Message::assistant()
                .with_tool_request("tool_1", Ok(CallToolRequestParams::new("render"))),
            Message::user().with_tool_response("tool_1", Ok(CallToolResult::success(vec![image]))),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec[1]["content"][0]["content"], "[Image: image/svg+xml]");
    }

    #[test]
    fn test_args_to_input_value_returns_empty_object_for_none() {
        let value = args_to_input_value(None);
        assert!(value.is_object(), "expected JSON object, got {value:?}");
        assert_eq!(value, json!({}));
        assert!(!value.is_null());
    }

    #[test]
    fn test_unparseable_tool_request_emits_placeholder_tool_use() {
        use rmcp::model::{ErrorCode, ErrorData};

        let err = ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "Tool arguments for id call_bad must be a JSON object".to_string(),
            None,
        );
        let mut response = Message::user();
        response.add_tool_response_with_metadata("call_bad", Err(err.clone()), None);
        let messages = vec![
            Message::assistant().with_tool_request("call_bad", Err(err)),
            response,
        ];

        let spec = format_messages(&messages);

        let mut open = std::collections::HashSet::new();
        for m in &spec {
            for block in m["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("tool_use") => {
                        open.insert(block["id"].as_str().unwrap().to_string());
                    }
                    Some("tool_result") => {
                        let id = block["tool_use_id"].as_str().unwrap();
                        assert!(open.contains(id), "orphan tool_result for id {id:?}");
                    }
                    _ => {}
                }
            }
        }
        assert!(open.contains("call_bad"));
    }

    #[test]
    fn test_args_to_input_value_preserves_existing_args() {
        let args = object!({"query": "rust"});
        let value = args_to_input_value(Some(args));
        assert_eq!(value, json!({"query": "rust"}));
    }

    #[test]
    fn test_parameterless_tool_request_serializes_input_as_empty_object() {
        // Regression test for #9287: when arguments is None (parameterless
        // MCP tool, session reload, or provider switching) the `input` field
        // must serialize as `{}` so the Anthropic API does not reject the
        // replayed tool_use block with a 400 error.
        let messages = vec![
            Message::assistant()
                .with_tool_request("tool_1", Ok(CallToolRequestParams::new("list_things"))),
            Message::user()
                .with_tool_response("tool_1", Ok(rmcp::model::CallToolResult::success(vec![]))),
        ];

        let spec = format_messages(&messages);

        let input = &spec[0]["content"][0]["input"];
        assert!(input.is_object(), "expected object, got {input:?}");
        assert!(!input.is_null());
        assert_eq!(input, &json!({}));
    }

    fn cfg(name: &str) -> ModelConfig {
        ModelConfig::new(name)
    }

    fn cfg_with_effort(name: &str, effort: &str) -> ModelConfig {
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), json!(effort));
        ModelConfig::new(name).with_merged_request_params(params)
    }

    #[test]
    fn test_thinking_type_from_effort() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);
        // Adaptive model with effort → adaptive
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-opus-4-6", "high")),
            ThinkingType::Adaptive
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-opus-4-7", "high")),
            ThinkingType::Adaptive
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-opus-4-8", "high")),
            ThinkingType::Adaptive
        );
        // Adaptive model with off → disabled
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-opus-4-6", "off")),
            ThinkingType::Disabled
        );
        // Non-adaptive Claude with effort → enabled
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-sonnet-4-5-20250929", "high")),
            ThinkingType::Enabled
        );
        // Non-adaptive Claude with off → disabled
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-sonnet-4-5-20250929", "off")),
            ThinkingType::Disabled
        );
    }

    #[test]
    fn test_thinking_type_always_on_adaptive() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);

        assert_eq!(
            thinking_type(&cfg("claude-fable-5")),
            ThinkingType::Adaptive
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-fable-5", "off")),
            ThinkingType::Adaptive
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-fable-5", "high")),
            ThinkingType::Adaptive
        );
    }

    #[test]
    fn test_create_request_fable_5_omits_temperature() -> Result<()> {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);
        let mut config = cfg("claude-fable-5");
        config.max_tokens = Some(4096);
        config.temperature = Some(0.7);
        let messages = vec![Message::user().with_text("Hello")];

        let payload = create_request_with_default_options(&config, "system", &messages, &[])?;

        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert!(payload.get("temperature").is_none());
        assert_eq!(payload["output_config"]["effort"], "high");

        Ok(())
    }

    #[test]
    fn test_thinking_type_non_claude_always_disabled() {
        assert_eq!(
            thinking_type(&cfg_with_effort("gpt-4o", "off")),
            ThinkingType::Disabled
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("gpt-4o", "high")),
            ThinkingType::Disabled
        );
    }

    #[test]
    fn test_thinking_type_off_means_disabled() {
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-opus-4-6", "off")),
            ThinkingType::Disabled
        );
        assert_eq!(
            thinking_type(&cfg_with_effort("claude-sonnet-4-5-20250929", "off")),
            ThinkingType::Disabled
        );
    }

    #[derive(Default)]
    struct StreamedParts {
        thinking: Vec<(String, String)>,
        redacted_thinking: Vec<String>,
        text: Vec<String>,
        tool_calls: Vec<String>,
        tool_errors: Vec<String>,
        output_token_limit_message_ids: Vec<Option<String>>,
    }

    async fn collect_stream(events: &str) -> StreamedParts {
        let mut parts = StreamedParts::default();

        for result in collect_stream_results(events).await {
            if let Ok((Some(msg), _usage)) = result {
                if msg.metadata.output_token_limit_reached {
                    parts.output_token_limit_message_ids.push(msg.id.clone());
                }
                for c in &msg.content {
                    match c {
                        MessageContentBlock::Thinking(t) => {
                            parts
                                .thinking
                                .push((t.thinking.clone(), t.signature.clone()));
                        }
                        MessageContentBlock::RedactedThinking(r) => {
                            parts.redacted_thinking.push(r.data.clone());
                        }
                        MessageContentBlock::Text(t) => {
                            parts.text.push(t.text.clone());
                        }
                        MessageContentBlock::ToolRequest(req) => match &req.tool_call {
                            Ok(call) => parts.tool_calls.push(call.name.to_string()),
                            Err(e) => parts.tool_errors.push(e.message.to_string()),
                        },
                        _ => {}
                    }
                }
            }
        }
        parts
    }

    #[tokio::test]
    async fn test_streaming_marks_max_tokens() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_limit","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Partial answer"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":25}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.text, vec!["Partial answer"]);
        assert_eq!(
            parts.output_token_limit_message_ids,
            vec![Some("msg_limit".to_string())]
        );
    }

    #[tokio::test]
    async fn test_streaming_thinking_and_text() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me analyze"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" this problem."}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_abc"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"123"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Here is the answer."}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":25}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.thinking.len(), 1);
        assert_eq!(parts.thinking[0].0, "Let me analyze this problem.");
        assert_eq!(parts.thinking[0].1, "sig_abc123");
        assert_eq!(parts.text, vec!["Here is the answer."]);
    }

    #[tokio::test]
    async fn test_streaming_thinking_from_start_block_without_signature() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"glm-4.7","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"Initial reasoning "}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"continues."}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.thinking.len(), 1);
        assert_eq!(parts.thinking[0].0, "Initial reasoning continues.");
        assert_eq!(parts.thinking[0].1, "");
    }

    #[tokio::test]
    async fn test_streaming_redacted_thinking() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_2","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":5,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"opaque_base64_data"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Done."}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.redacted_thinking, vec!["opaque_base64_data"]);
        assert_eq!(parts.text, vec!["Done."]);
    }

    #[tokio::test]
    async fn test_streaming_thinking_text_then_tool_call() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_3","role":"assistant","content":[],"model":"claude-sonnet-4-6","usage":{"input_tokens":8,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"I should search for this."}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"tool_sig_xyz"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Let me search for that."}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool_1","name":"search","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"rust\"}"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":2}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.thinking.len(), 1);
        assert_eq!(
            parts.thinking[0],
            (
                "I should search for this.".to_string(),
                "tool_sig_xyz".to_string()
            )
        );
        assert_eq!(parts.text, vec!["Let me search for that."]);
        assert_eq!(parts.tool_calls, vec!["search"]);
    }

    async fn collect_stream_results(
        events: &str,
    ) -> Vec<anyhow::Result<(Option<Message>, Option<ProviderUsage>)>> {
        use futures::StreamExt;

        let lines: Vec<Result<String, anyhow::Error>> =
            events.lines().map(|l| Ok(l.to_string())).collect();
        let stream = Box::pin(futures::stream::iter(lines));
        response_to_streaming_message(stream).collect().await
    }

    #[tokio::test]
    async fn test_streaming_reassembles_interleaved_parallel_tool_calls() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_par","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":5,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_a","name":"search","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_b","name":"write","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/tmp/a.md\"}"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"rust\"}"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let requests = collect_tool_requests(events).await;

        assert_eq!(requests.len(), 2);
        let write = requests
            .iter()
            .find(|r| r.id == "tool_b")
            .expect("write tool request");
        assert_eq!(write.provider_index(), Some(1));
        let write_call = write.tool_call.as_ref().expect("write args parsed");
        assert_eq!(write_call.name, "write");
        assert_eq!(
            write_call.arguments.as_ref().unwrap()["path"],
            json!("/tmp/a.md")
        );

        let search = requests
            .iter()
            .find(|r| r.id == "tool_a")
            .expect("search tool request");
        assert_eq!(search.provider_index(), Some(0));
        let search_call = search.tool_call.as_ref().expect("search args parsed");
        assert_eq!(search_call.name, "search");
        assert_eq!(
            search_call.arguments.as_ref().unwrap()["query"],
            json!("rust")
        );
    }

    async fn collect_tool_requests(events: &str) -> Vec<crate::conversation::message::ToolRequest> {
        let mut requests = Vec::new();
        for result in collect_stream_results(events).await {
            if let Ok((Some(msg), _usage)) = result {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(req) = content {
                        requests.push(req.clone());
                    }
                }
            }
        }
        requests
    }

    #[tokio::test]
    async fn test_streaming_preserves_cache_tokens_through_delta_merge() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":7,"cache_creation_input_tokens":10000,"cache_read_input_tokens":5000,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":25}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let usage = collect_stream_results(events)
            .await
            .into_iter()
            .filter_map(|r| r.ok().and_then(|(_, usage)| usage))
            .next_back()
            .expect("stream should yield usage");

        assert_eq!(usage.usage.input_tokens, Some(15007));
        assert_eq!(usage.usage.output_tokens, Some(25));
        assert_eq!(usage.usage.cache_read_input_tokens, Some(5000));
        assert_eq!(usage.usage.cache_write_input_tokens, Some(10000));
        assert_eq!(
            usage.finish_reasons.as_deref(),
            Some(&["end_turn".to_string()][..])
        );
        assert_eq!(usage.response_id.as_deref(), Some("msg_1"));
    }

    async fn streamed_usage(events: &str) -> ProviderUsage {
        collect_stream_results(events)
            .await
            .into_iter()
            .filter_map(|r| r.ok().and_then(|(_, usage)| usage))
            .next_back()
            .expect("stream should yield usage")
    }

    #[tokio::test]
    async fn test_streaming_surfaces_additional_usage_data() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-sonnet-4-5","usage":{"input_tokens":7,"output_tokens":0,"service_tier":"fast"}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":25}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let additional = streamed_usage(events)
            .await
            .additional_data
            .expect("additional data should be reported");
        assert_eq!(additional["service_tier"], json!("fast"));
    }

    #[tokio::test]
    async fn test_streaming_omits_additional_usage_data_when_absent() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-sonnet-4-5","usage":{"input_tokens":7,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":25}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        assert!(streamed_usage(events).await.additional_data.is_none());
    }

    #[tokio::test]
    async fn test_streaming_preserves_provider_cost_from_delta() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"m1","role":"assistant","content":[],"model":"glm-4.7","usage":{"input_tokens":100,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50,"cost":0.0123}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let usage = collect_stream_results(events)
            .await
            .into_iter()
            .filter_map(|r| r.ok().and_then(|(_, usage)| usage))
            .next_back()
            .expect("stream should yield usage");

        assert_eq!(usage.cost, Some(0.0123));
        assert_eq!(usage.cost_source, Some(CostSource::ProviderReported));
        assert_eq!(usage.usage.input_tokens, Some(100));
        assert_eq!(usage.usage.output_tokens, Some(50));
    }

    #[tokio::test]
    async fn test_streaming_delta_usage_is_cumulative_and_wins() {
        // Server tool use grows input during the turn: the final
        // message_delta usage is authoritative, not message_start.
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":2679,"cache_creation_input_tokens":100,"cache_read_input_tokens":200,"output_tokens":3}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":10682,"cache_creation_input_tokens":100,"cache_read_input_tokens":200,"output_tokens":510}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let usage = collect_stream_results(events)
            .await
            .into_iter()
            .filter_map(|r| r.ok().and_then(|(_, usage)| usage))
            .next_back()
            .expect("stream should yield usage");

        assert_eq!(usage.usage.input_tokens, Some(10982)); // 10682 + 100 + 200
        assert_eq!(usage.usage.output_tokens, Some(510));
        assert_eq!(usage.usage.cache_read_input_tokens, Some(200));
        assert_eq!(usage.usage.cache_write_input_tokens, Some(100));
    }

    #[test]
    fn test_merge_delta_usage_raw_input_inherits_start_cache() {
        let start =
            Usage::new(Some(15007), Some(3), None).with_cache_tokens(Some(5000), Some(10000));
        let delta_data = json!({"input_tokens": 8, "output_tokens": 510});
        let delta = get_usage(&delta_data).unwrap();

        let merged = merge_delta_usage(&start, &delta, &delta_data);
        assert_eq!(merged.input_tokens, Some(15008)); // 8 + 5000 + 10000
        assert_eq!(merged.output_tokens, Some(510));
        assert_eq!(merged.cache_read_input_tokens, Some(5000));
        assert_eq!(merged.cache_write_input_tokens, Some(10000));
    }

    fn expect_refusal(
        results: Vec<anyhow::Result<(Option<Message>, Option<ProviderUsage>)>>,
    ) -> (String, Option<String>) {
        let err = results
            .into_iter()
            .find_map(|r| r.err())
            .expect("refusal should surface as a stream error");
        match err.downcast_ref::<ProviderError>() {
            Some(ProviderError::Refusal { details, category }) => {
                (details.clone(), category.clone())
            }
            other => panic!("expected ProviderError::Refusal, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_streaming_refusal() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"explanation":"This request violates the usage policy.","category":"cyber"}},"usage":{"output_tokens":5}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let results = collect_stream_results(events).await;
        let usage = results
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .find_map(|(_, usage)| usage.clone())
            .expect("a refused request should still yield its usage");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(5));
        assert_eq!(usage.finish_reasons, Some(vec!["refusal".to_string()]));
        assert_eq!(usage.response_id.as_deref(), Some("msg_1"));

        let (details, category) = expect_refusal(results);
        assert_eq!(details, "This request violates the usage policy.");
        assert_eq!(category.as_deref(), Some("cyber"));
    }

    #[tokio::test]
    async fn test_streaming_refusal_forwards_unrecognized_stop_details() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"code":42}},"usage":{"output_tokens":5}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let (details, category) = expect_refusal(collect_stream_results(events).await);
        assert!(details.contains("\"code\":42"), "details: {details}");
        assert_eq!(category, None);
    }

    #[tokio::test]
    async fn test_streaming_forwards_unhandled_stop_details() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"model_context_window_exceeded","stop_details":{"reason":"context_window"}},"usage":{"output_tokens":5}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.text.len(), 1);
        assert!(
            parts.text[0].starts_with("The provider ended the response with:"),
            "text: {}",
            parts.text[0]
        );
        assert!(parts.text[0].contains("context_window"));
    }

    #[tokio::test]
    async fn test_streaming_truncated_tool_args_in_content_block_stop() {
        // Block is closed by content_block_stop, but the concatenated deltas form
        // truncated JSON (each fragment is valid; together they're unterminated).
        let events = concat!(
            r##"data: {"type":"message_start","message":{"id":"msg_t","role":"assistant","content":[],"model":"glm-4.7","usage":{"input_tokens":10,"output_tokens":0}}}"##,
            "\n",
            r##"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_t","name":"write","input":{}}}"##,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/some/path.md\","}}"#,
            "\n",
            r##"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"content\":\"# Very long markdown"}}"##,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":4096}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(
            parts.tool_errors.len(),
            1,
            "expected one tool error, got: {:?}",
            parts.tool_errors
        );
        let msg = &parts.tool_errors[0];
        assert!(
            msg.contains("truncated") || msg.contains("output token limit"),
            "expected actionable truncation message, got: {}",
            msg
        );
        assert!(
            msg.contains("max_tokens") || msg.contains("smaller steps"),
            "expected guidance to increase max_tokens or break up the task, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_streaming_truncated_tool_args_no_content_block_stop() {
        // The stream ends with the tool_use block still open (no content_block_stop),
        // which is what happens when the model is cut off mid-tool-call.
        let events = concat!(
            r##"data: {"type":"message_start","message":{"id":"msg_t2","role":"assistant","content":[],"model":"glm-4.7","usage":{"input_tokens":10,"output_tokens":0}}}"##,
            "\n",
            r##"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_t2","name":"write","input":{}}}"##,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/report.md\","}}"#,
            "\n",
            r##"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"content\":\"# Big report that got cut off mid"}}"##,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":8192}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(
            parts.output_token_limit_message_ids,
            vec![Some("msg_t2".to_string())]
        );
        assert_eq!(
            parts.tool_errors.len(),
            1,
            "expected one tool error for the dropped/truncated tool call, got: {:?}",
            parts.tool_errors
        );
        let msg = &parts.tool_errors[0];
        assert!(
            msg.contains("truncated") || msg.contains("output token limit"),
            "expected actionable truncation message, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_streaming_unfinished_tool_calls_keep_provider_indices() {
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_t3","role":"assistant","content":[],"model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_open_a","name":"search","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_open_b","name":"write","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"ru"}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/re"}}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":8192}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let requests = collect_tool_requests(events).await;

        assert_eq!(requests.len(), 2);
        let indexed: Vec<(String, Option<i32>)> = requests
            .iter()
            .map(|r| (r.id.clone(), r.provider_index()))
            .collect();
        assert_eq!(
            indexed,
            vec![
                ("tool_open_a".to_string(), Some(0)),
                ("tool_open_b".to_string(), Some(1)),
            ]
        );
        assert!(requests.iter().all(|r| r.tool_call.is_err()));
    }

    #[tokio::test]
    async fn test_streaming_complete_tool_call_unaffected() {
        // Regression guard: a normal, complete tool call must still parse and
        // produce no error even though stop_reason handling is added.
        let events = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_ok","role":"assistant","content":[],"model":"glm-4.7","usage":{"input_tokens":10,"output_tokens":0}}}"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_ok","name":"write","input":{}}}"#,
            "\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/ok.md\",\"content\":\"hello\"}"}}"#,
            "\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#,
            "\n",
            r#"data: {"type":"message_stop"}"#,
        );

        let parts = collect_stream(events).await;
        assert_eq!(parts.tool_calls, vec!["write"]);
        assert!(parts.tool_errors.is_empty());
    }

    /// Placement shape only; cross-request prefix stability is covered by
    /// `tests/prefix_invariance.rs`.
    mod cache_breakpoint_placement {
        use super::*;
        use rmcp::model::CallToolResult;

        fn sample_tools() -> Vec<Tool> {
            vec![
                Tool::new(
                    "read_file",
                    "Read a file from disk",
                    object!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } }
                    }),
                ),
                Tool::new(
                    "write_file",
                    "Write a file to disk",
                    object!({
                        "type": "object",
                        "properties": { "path": { "type": "string" }, "content": { "type": "string" } }
                    }),
                ),
            ]
        }

        fn breakpoints(messages: &[Value]) -> Vec<(usize, usize)> {
            let mut found = Vec::new();
            for (mi, message) in messages.iter().enumerate() {
                for (bi, block) in message["content"].as_array().unwrap().iter().enumerate() {
                    if block.get(CACHE_CONTROL_FIELD).is_some() {
                        found.push((mi, bi));
                    }
                }
            }
            found
        }

        #[test]
        fn breakpoints_cover_tools_system_and_last_two_user_messages() {
            let messages = vec![
                Message::user().with_text("What does the main entrypoint do?"),
                Message::assistant().with_tool_request(
                    "tool_1",
                    Ok(CallToolRequestParams::new("read_file")
                        .with_arguments(object!({"path": "src/main.rs"}))),
                ),
                Message::user().with_tool_response(
                    "tool_1",
                    Ok(CallToolResult::success(vec![
                        rmcp::model::ContentBlock::text("fn main() { run(); }"),
                    ])),
                ),
                Message::assistant().with_text("It calls `run()`."),
                Message::user().with_text("Now add error handling to it."),
            ];
            let req = create_request_with_default_options(
                &cfg("claude-sonnet-4-5"),
                "You are a careful coding assistant.",
                &messages,
                &sample_tools(),
            )
            .unwrap();

            let tools = req["tools"].as_array().unwrap();
            assert!(tools[0].get(CACHE_CONTROL_FIELD).is_none());
            assert!(tools[1].get(CACHE_CONTROL_FIELD).is_some());
            assert!(req["system"][0].get(CACHE_CONTROL_FIELD).is_some());

            let request_messages = req["messages"].as_array().unwrap();
            let marked = breakpoints(request_messages);
            assert_eq!(
                marked,
                vec![(2, 0), (4, 0)],
                "message breakpoints should sit on the last block of the last two user messages"
            );
        }

        #[test]
        fn disable_prompt_cache_removes_every_breakpoint() {
            let config = cfg("claude-sonnet-4-5").with_merged_request_params(
                std::collections::HashMap::from([(
                    "disable_prompt_cache".to_string(),
                    json!(true),
                )]),
            );
            let req = create_request_with_default_options(
                &config,
                "You are a summarizer.",
                &[Message::user().with_text("Summarize the conversation above.")],
                &sample_tools(),
            )
            .unwrap();

            assert!(!req.to_string().contains(CACHE_CONTROL_FIELD));
        }

        fn cache_control_values(req: &Value) -> Vec<Value> {
            let mut found = Vec::new();
            let tools = req["tools"].as_array().unwrap();
            let system = req["system"].as_array().unwrap();
            let messages = req["messages"].as_array().unwrap();
            for block in tools
                .iter()
                .chain(system.iter())
                .chain(messages.iter().flat_map(|m| {
                    m["content"]
                        .as_array()
                        .map(|c| c.iter())
                        .unwrap_or_default()
                }))
            {
                if let Some(cc) = block.get(CACHE_CONTROL_FIELD) {
                    found.push(cc.clone());
                }
            }
            found
        }

        #[test]
        fn default_breakpoints_omit_ttl() {
            let req = create_request_with_default_options(
                &cfg("claude-sonnet-4-5"),
                "You are a careful coding assistant.",
                &[Message::user().with_text("Hello")],
                &sample_tools(),
            )
            .unwrap();

            let values = cache_control_values(&req);
            assert_eq!(values.len(), 3);
            for cc in values {
                assert_eq!(cc, json!({ "type": "ephemeral" }));
            }
        }

        #[test]
        fn one_hour_ttl_stamps_every_breakpoint() {
            let config = cfg("claude-sonnet-4-5").with_cache_ttl("1h");
            let req = create_request_with_default_options(
                &config,
                "You are a careful coding assistant.",
                &[
                    Message::user().with_text("Hello"),
                    Message::assistant().with_text("Hi."),
                    Message::user().with_text("Continue"),
                ],
                &sample_tools(),
            )
            .unwrap();

            let values = cache_control_values(&req);
            assert_eq!(values.len(), 4);
            for cc in values {
                assert_eq!(cc, json!({ "type": "ephemeral", "ttl": "1h" }));
            }
        }

        #[test]
        fn explicit_five_minute_ttl_matches_default_wire_format() {
            let config = cfg("claude-sonnet-4-5").with_cache_ttl("5m");
            let req = create_request_with_default_options(
                &config,
                "You are a careful coding assistant.",
                &[Message::user().with_text("Hello")],
                &sample_tools(),
            )
            .unwrap();

            for cc in cache_control_values(&req) {
                assert_eq!(cc, json!({ "type": "ephemeral" }));
            }
        }

        #[test]
        fn unrecognized_ttl_value_falls_back_to_default() {
            let config = cfg("claude-sonnet-4-5").with_cache_ttl("2h");
            let req = create_request_with_default_options(
                &config,
                "You are a careful coding assistant.",
                &[Message::user().with_text("Hello")],
                &sample_tools(),
            )
            .unwrap();

            for cc in cache_control_values(&req) {
                assert_eq!(cc, json!({ "type": "ephemeral" }));
            }
        }

        #[test]
        fn disable_prompt_cache_wins_over_ttl() {
            let config = cfg("claude-sonnet-4-5")
                .with_cache_ttl("1h")
                .with_prompt_cache_disabled();
            let req = create_request_with_default_options(
                &config,
                "You are a summarizer.",
                &[Message::user().with_text("Summarize.")],
                &sample_tools(),
            )
            .unwrap();

            assert!(!req.to_string().contains(CACHE_CONTROL_FIELD));
        }

        #[test]
        fn breakpoints_land_on_the_last_content_block() {
            let messages = vec![Message::user()
                .with_text("Here is the context.")
                .with_text("And the question.")];
            let req = create_request_with_default_options(
                &cfg("claude-sonnet-4-5"),
                "You are a careful coding assistant.",
                &messages,
                &sample_tools(),
            )
            .unwrap();

            let request_messages = req["messages"].as_array().unwrap();
            assert_eq!(breakpoints(request_messages), vec![(0, 1)]);
        }
    }
}
